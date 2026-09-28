//! The risk engine. Every execution path — sniper, copy trader, Polymarket —
//! must call [`RiskEngine::check_entry`] before building an order and
//! [`RiskEngine::check_exit`] on every price update.
//!
//! The engine is deliberately conservative: anything it cannot verify is
//! rejected, and the kill switch short-circuits every check.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::config::RiskConfig;
use crate::models::{BotModule, Position, TokenLaunch, Venue, WalletTrade};
use crate::state::Shared;

/// Cluster-wide risk view (Prompt 3 gap closure for §Q items 3/4). When
/// attached (server-side, Postgres-backed), the risk engine consults the
/// shared store IN ADDITION to the eventually-consistent local book and
/// daily accumulator, so global limits converge within one query instead of
/// within `[ha].book_sync_secs`.
///
/// Rules:
/// * every method returns `None` = "unknown" (store unavailable) — the
///   engine then falls back to the local-only view; risk checks must never
///   DEPEND on store availability (§K);
/// * values combine conservatively: `max` for open-position counts, `min`
///   (more negative wins) for realized PnL — an oracle can only tighten a
///   limit, never loosen it.
#[async_trait]
pub trait GlobalRiskOracle: Send + Sync {
    /// Open positions across ALL replicas for `module`.
    async fn count_open(&self, module: BotModule) -> Option<usize>;
    /// Today's (UTC) realized PnL summed across ALL replicas.
    async fn realized_today(&self) -> Option<f64>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskVerdict {
    /// Proceed, and use `sized_quote` as the order size.
    Allow,
    /// Proceed but with a reduced size (already reflected in `sized_quote`).
    AllowReduced,
    /// Do not trade.
    Reject,
}

/// Machine-readable classification of a rejection. `reason` stays the
/// human explanation; the code lets callers (sniper pipeline, dashboards,
/// replay fixtures) branch and count without parsing text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskCode {
    KillSwitch,
    ModuleDisabled,
    DailyLossLimit,
    InvalidSize,
    SlippageCap,
    MaxOpenPositions,
    DuplicateSymbol,
    ReentryCooldown,
    InsufficientBalance,
    ExposureCap,
    VenueCheck,
    /// `risk.sniper_emergency_disable` is set (sniper entries only).
    SniperEmergencyDisabled,
    /// `risk.sniper_daily_loss_limit_quote` reached (sniper entries only).
    SniperDailyLoss,
    /// `risk.sniper_max_pending_executions` live intents in the ledger.
    SniperPendingCap,
    /// `risk.sniper_token_cooldown_secs` since the last attempt on the mint.
    SniperTokenCooldown,
    /// `risk.sniper_failed_entry_cooldown_secs` since the last failed entry.
    SniperFailedEntryCooldown,
    /// `risk.copy_emergency_disable` is set (copy entries only).
    CopyEmergencyDisabled,
    /// `risk.copy_daily_loss_limit_quote` reached (copy entries only).
    CopyDailyLoss,
    /// `risk.copy_max_pending_executions` live copy intents in the ledger.
    CopyPendingCap,
    /// `risk.copy_failed_entry_cooldown_secs` since the last failed entry.
    CopyFailedEntryCooldown,
    /// Open exposure mirrored from one leader would exceed its cap.
    CopyLeaderExposure,
    /// `risk.copy_cooldown_secs` since this leader was last copied on the mint.
    CopyCooldown,
    /// The leader trade is older than the copy engine is willing to mirror.
    StaleSignal,
    /// `risk.poly_emergency_disable` is set (Polymarket entries only).
    PolyEmergencyDisabled,
    /// `risk.poly_daily_loss_limit_quote` reached (Polymarket entries only).
    PolyDailyLoss,
    /// `risk.poly_max_open_orders` resting CLOB orders already exist.
    PolyOpenOrderCap,
    /// Open exposure inside one market (positions + resting orders) would
    /// exceed `risk.poly_max_market_exposure_quote`.
    PolyMarketExposure,
    /// TASK 5 — a venue / strategy kill switch of the global risk engine.
    GlobalKillSwitch,
    /// TASK 5 — a portfolio / wallet / venue / strategy / asset exposure
    /// cap, the global open-position cap or the per-order notional cap.
    GlobalExposure,
    /// TASK 5 — `[global_risk].max_daily_loss_ref` reached.
    GlobalDailyLoss,
    /// TASK 5 — `[global_risk].max_drawdown_ref` / `max_drawdown_pct` reached.
    GlobalDrawdown,
    /// TASK 5 — the global engine could not evaluate its limits (no
    /// reference rate for the quote asset) or the request was malformed;
    /// fail closed.
    GlobalUnavailable,
}

impl RiskCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            RiskCode::KillSwitch => "kill_switch",
            RiskCode::ModuleDisabled => "module_disabled",
            RiskCode::DailyLossLimit => "daily_loss_limit",
            RiskCode::InvalidSize => "invalid_size",
            RiskCode::SlippageCap => "slippage_cap",
            RiskCode::MaxOpenPositions => "max_open_positions",
            RiskCode::DuplicateSymbol => "duplicate_symbol",
            RiskCode::ReentryCooldown => "reentry_cooldown",
            RiskCode::InsufficientBalance => "insufficient_balance",
            RiskCode::ExposureCap => "exposure_cap",
            RiskCode::VenueCheck => "venue_check",
            RiskCode::SniperEmergencyDisabled => "sniper_emergency_disabled",
            RiskCode::SniperDailyLoss => "sniper_daily_loss",
            RiskCode::SniperPendingCap => "sniper_pending_cap",
            RiskCode::SniperTokenCooldown => "sniper_token_cooldown",
            RiskCode::SniperFailedEntryCooldown => "sniper_failed_entry_cooldown",
            RiskCode::CopyEmergencyDisabled => "copy_emergency_disabled",
            RiskCode::CopyDailyLoss => "copy_daily_loss",
            RiskCode::CopyPendingCap => "copy_pending_cap",
            RiskCode::CopyFailedEntryCooldown => "copy_failed_entry_cooldown",
            RiskCode::CopyLeaderExposure => "copy_leader_exposure",
            RiskCode::CopyCooldown => "copy_cooldown",
            RiskCode::StaleSignal => "stale_signal",
            RiskCode::PolyEmergencyDisabled => "poly_emergency_disabled",
            RiskCode::PolyDailyLoss => "poly_daily_loss",
            RiskCode::PolyOpenOrderCap => "poly_open_order_cap",
            RiskCode::PolyMarketExposure => "poly_market_exposure",
            RiskCode::GlobalKillSwitch => "global_kill_switch",
            RiskCode::GlobalExposure => "global_exposure",
            RiskCode::GlobalDailyLoss => "global_daily_loss",
            RiskCode::GlobalDrawdown => "global_drawdown",
            RiskCode::GlobalUnavailable => "global_unavailable",
        }
    }

    /// Map a global-engine reason onto the module-facing code.
    pub fn from_global(reason: crate::global_risk::GlobalRejectReason) -> RiskCode {
        use crate::global_risk::GlobalRejectReason as G;
        match reason {
            G::GlobalKillSwitch | G::VenueKillSwitch | G::StrategyKillSwitch => {
                RiskCode::GlobalKillSwitch
            }
            G::MaxOpenPositions
            | G::OrderNotional
            | G::PortfolioExposure
            | G::WalletExposure
            | G::VenueExposure
            | G::StrategyExposure
            | G::AssetExposure => RiskCode::GlobalExposure,
            G::DailyLoss => RiskCode::GlobalDailyLoss,
            G::Drawdown => RiskCode::GlobalDrawdown,
            G::ReferenceRateMissing | G::InvalidRequest => RiskCode::GlobalUnavailable,
        }
    }

    /// True for the codes that mean "capacity/exposure is used up" rather
    /// than "this particular request is bad".
    pub fn is_exposure_limit(&self) -> bool {
        matches!(
            self,
            RiskCode::MaxOpenPositions
                | RiskCode::ExposureCap
                | RiskCode::DailyLossLimit
                | RiskCode::SniperDailyLoss
                | RiskCode::SniperPendingCap
                | RiskCode::CopyDailyLoss
                | RiskCode::CopyPendingCap
                | RiskCode::CopyLeaderExposure
                | RiskCode::PolyDailyLoss
                | RiskCode::PolyOpenOrderCap
                | RiskCode::PolyMarketExposure
                | RiskCode::GlobalExposure
                | RiskCode::GlobalDailyLoss
                | RiskCode::GlobalDrawdown
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RiskDecision {
    pub verdict: RiskVerdict,
    pub reason: String,
    /// Set on every rejection produced by [`RiskEngine::check_entry`];
    /// `None` on allow verdicts.
    pub code: Option<RiskCode>,
    /// Order size in quote units (SOL for Solana venues, USDC for Polymarket).
    pub sized_quote: f64,
    pub requested_quote: f64,
    pub stop_loss: Option<f64>,
    pub take_profit: Option<f64>,
    pub trailing_stop_pct: Option<f64>,
    pub max_hold_secs: Option<i64>,
}

impl RiskDecision {
    pub fn allowed(&self) -> bool {
        !matches!(self.verdict, RiskVerdict::Reject)
    }

    /// Copy this decision's exit parameters onto a freshly opened position.
    ///
    /// Callers MUST do this, otherwise `check_exit` falls back to the global
    /// defaults and any per-trade tuning is silently ignored.
    pub fn apply_to(&self, position: &mut Position) {
        if let Some(sl) = self.stop_loss {
            position.stop_loss = Some(sl);
        }
        if let Some(tp) = self.take_profit {
            position.take_profit = Some(tp);
        }
        if position.trailing_stop.is_none() {
            position.trailing_stop = self.trailing_stop_pct;
        }
        if position.max_hold_secs.is_none() {
            position.max_hold_secs = self.max_hold_secs;
        }
    }

    fn reject<S: Into<String>>(code: RiskCode, requested: f64, reason: S) -> Self {
        RiskDecision {
            verdict: RiskVerdict::Reject,
            reason: reason.into(),
            code: Some(code),
            sized_quote: 0.0,
            requested_quote: requested,
            stop_loss: None,
            take_profit: None,
            trailing_stop_pct: None,
            max_hold_secs: None,
        }
    }
}

/// Which exit rule fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitRule {
    StopLoss,
    TakeProfit,
    TrailingStop,
    MaxHoldTime,
    KillSwitch,
    Manual,
    /// The wallet we were copying sold out.
    MirrorExit,
    /// The position's mark price could not be refreshed for longer than
    /// `sniper.stale_position_exit_secs` — flatten blind rather than hold
    /// an unpriceable token forever.
    StalePosition,
}

impl ExitRule {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExitRule::StopLoss => "stop_loss",
            ExitRule::TakeProfit => "take_profit",
            ExitRule::TrailingStop => "trailing_stop",
            ExitRule::MaxHoldTime => "max_hold_time",
            ExitRule::KillSwitch => "kill_switch",
            ExitRule::Manual => "manual",
            ExitRule::MirrorExit => "mirror_exit",
            ExitRule::StalePosition => "stale_position",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ExitDecision {
    pub should_exit: bool,
    pub rule: Option<ExitRule>,
    /// Fraction of the position to sell (1.0 = all of it).
    pub fraction: f64,
    pub reason: String,
}

impl ExitDecision {
    fn hold() -> Self {
        ExitDecision {
            should_exit: false,
            rule: None,
            fraction: 0.0,
            reason: "hold".into(),
        }
    }

    fn exit(rule: ExitRule, fraction: f64, reason: impl Into<String>) -> Self {
        ExitDecision {
            should_exit: true,
            rule: Some(rule),
            fraction: fraction.clamp(0.0, 1.0),
            reason: reason.into(),
        }
    }

    /// A full exit forced by an out-of-band rule (stale mark, operator
    /// action). Exposed so module sweepers can express decisions the price
    /// rules cannot make while keeping one `ExitDecision` shape.
    pub fn forced(rule: ExitRule, reason: impl Into<String>) -> Self {
        ExitDecision::exit(rule, 1.0, reason)
    }
}

/// Everything the engine needs to evaluate one candidate entry.
#[derive(Debug, Clone)]
pub struct EntryRequest {
    pub module: BotModule,
    pub venue: Venue,
    pub symbol: String,
    pub symbol_display: String,
    /// Quote units the strategy wants to spend.
    pub requested_quote: f64,
    /// Current wallet balance in quote units.
    pub available_quote: f64,
    /// Slippage the strategy intends to allow, in basis points.
    pub slippage_bps: u64,
    /// Set for Polymarket: price per share in [0, 1].
    pub price: Option<f64>,
    /// Set for Polymarket: our model's fair probability in [0, 1].
    pub fair_value: Option<f64>,
    /// Set for Polymarket: book liquidity in USDC.
    pub liquidity: Option<f64>,
    /// TASK 5 — our wallet / account the entry would trade from (Solana
    /// pubkey, Polygon EOA). Empty = unknown; the global engine then
    /// attributes the exposure to the module name.
    pub wallet: String,
    /// TASK 5 — strategy label the exposure is attributed to (`sniper`,
    /// `copy:<leader>`, the Polymarket strategy name). Empty = module name.
    pub strategy: String,
}

impl EntryRequest {
    /// Wallet label the global layer attributes the entry to.
    pub fn wallet_label(&self) -> String {
        let w = self.wallet.trim();
        if w.is_empty() {
            self.module.as_str().to_string()
        } else {
            w.to_string()
        }
    }

    /// Strategy label the global layer attributes the entry to.
    pub fn strategy_label(&self) -> String {
        let s = self.strategy.trim();
        if s.is_empty() {
            self.module.as_str().to_string()
        } else {
            s.to_string()
        }
    }

    /// The global-risk request for this entry.
    pub async fn global_request(&self, state: &Shared) -> crate::global_risk::GlobalRiskRequest {
        crate::global_risk::GlobalRiskRequest {
            module: self.module,
            venue: self.venue,
            wallet: self.wallet_label(),
            strategy: self.strategy_label(),
            asset: self.symbol.clone(),
            quote_asset: crate::global_risk::quote_asset_for(self.venue).to_string(),
            requested_quote: self.requested_quote,
            mode: state.execution_mode().await,
        }
    }
}

/// Static analysis of a launch, computed before any on-chain call.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TokenChecks {
    pub has_socials: bool,
    pub social_count: usize,
    pub creator_buy_sol: f64,
    pub market_cap_sol: f64,
    pub keyword_hit: Option<String>,
    pub creator_denied: bool,
    pub known_rug_creator: bool,
    pub too_old_secs: Option<i64>,
}

#[derive(Clone)]
pub struct RiskEngine {
    state: Shared,
}

impl RiskEngine {
    pub fn new(state: Shared) -> Self {
        RiskEngine { state }
    }

    pub fn state(&self) -> &Arc<crate::state::AppState> {
        &self.state
    }

    async fn risk_config(&self) -> RiskConfig {
        self.state.config.read().await.risk.clone()
    }

    /// Hard gate used by every sender: is trading permitted at all right now?
    pub async fn preflight(&self, module: BotModule) -> Result<(), String> {
        self.preflight_coded(module)
            .await
            .map_err(|(_, reason)| reason)
    }

    /// [`RiskEngine::preflight`] with the machine-readable code attached.
    pub async fn preflight_coded(&self, module: BotModule) -> Result<(), (RiskCode, String)> {
        if self.state.kill_switch() {
            return Err((RiskCode::KillSwitch, "kill switch engaged".into()));
        }
        if !self.state.is_enabled(module).await {
            return Err((RiskCode::ModuleDisabled, format!("{module} is disabled")));
        }
        let risk = self.risk_config().await;
        let daily = self.state.daily_stats().await;
        if daily.loss_limit_tripped {
            return Err((
                RiskCode::DailyLossLimit,
                format!(
                    "daily loss limit tripped at {}",
                    daily
                        .loss_limit_tripped_at
                        .map(|t| t.to_rfc3339())
                        .unwrap_or_else(|| "unknown".into())
                ),
            ));
        }
        if risk.daily_loss_limit_quote > 0.0 {
            let realized = self.effective_realized(daily.realized_pnl).await;
            if realized <= -risk.daily_loss_limit_quote {
                return Err((
                    RiskCode::DailyLossLimit,
                    format!(
                        "daily realized loss {:.4} exceeds limit {:.4}",
                        realized, risk.daily_loss_limit_quote
                    ),
                ));
            }
        }
        if module == BotModule::Sniper {
            if risk.sniper_emergency_disable {
                return Err((
                    RiskCode::SniperEmergencyDisabled,
                    "risk.sniper_emergency_disable is set — sniper entries refused".into(),
                ));
            }
            if risk.sniper_daily_loss_limit_quote > 0.0 {
                let sniper_realized = self.state.daily_realized(BotModule::Sniper).await;
                if sniper_realized <= -risk.sniper_daily_loss_limit_quote {
                    return Err((
                        RiskCode::SniperDailyLoss,
                        format!(
                            "sniper daily realized loss {:.4} exceeds sniper limit {:.4}",
                            sniper_realized, risk.sniper_daily_loss_limit_quote
                        ),
                    ));
                }
            }
        }
        if module == BotModule::Copy {
            if risk.copy_emergency_disable {
                return Err((
                    RiskCode::CopyEmergencyDisabled,
                    "risk.copy_emergency_disable is set — mirrored entries refused".into(),
                ));
            }
            if risk.copy_daily_loss_limit_quote > 0.0 {
                let copy_realized = self.state.daily_realized(BotModule::Copy).await;
                if copy_realized <= -risk.copy_daily_loss_limit_quote {
                    return Err((
                        RiskCode::CopyDailyLoss,
                        format!(
                            "copy daily realized loss {:.4} exceeds copy limit {:.4}",
                            copy_realized, risk.copy_daily_loss_limit_quote
                        ),
                    ));
                }
            }
        }
        if module == BotModule::Polymarket {
            if risk.poly_emergency_disable {
                return Err((
                    RiskCode::PolyEmergencyDisabled,
                    "risk.poly_emergency_disable is set — polymarket entries refused".into(),
                ));
            }
            if risk.poly_daily_loss_limit_quote > 0.0 {
                let poly_realized = self.state.daily_realized(BotModule::Polymarket).await;
                if poly_realized <= -risk.poly_daily_loss_limit_quote {
                    return Err((
                        RiskCode::PolyDailyLoss,
                        format!(
                            "polymarket daily realized loss {:.4} exceeds polymarket limit {:.4}",
                            poly_realized, risk.poly_daily_loss_limit_quote
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Sniper execution intents that are live in the process-wide execution
    /// ledger (created / validated / submitted / pending — anything not yet
    /// settled). Entries only: the sniper labels its buys `snipe-…`, its
    /// sells `exit-…`, and exits must never be throttled by this cap.
    pub async fn pending_sniper_entries(&self) -> usize {
        crate::execution::ledger()
            .open()
            .await
            .iter()
            .filter(|r| r.module == BotModule::Sniper.as_str() && r.label.starts_with("snipe"))
            .count()
    }

    /// Copy execution intents that are live in the process-wide execution
    /// ledger. Entries only: mirrored buys are labelled `copy-…` /
    /// `copy-jup-…`, exits `copy-exit-…`, and exits are never throttled.
    pub async fn pending_copy_entries(&self) -> usize {
        crate::execution::ledger()
            .open()
            .await
            .iter()
            .filter(|r| {
                r.module == BotModule::Copy.as_str()
                    && r.label.starts_with("copy-")
                    && !r.label.starts_with("copy-exit")
            })
            .count()
    }

    /// Conservative combination of the LOCAL daily realized PnL with the
    /// cluster-wide view (when an oracle is attached): the more negative of
    /// the two wins. Oracle failures keep the local view (§K: risk checks
    /// never depend on store availability).
    async fn effective_realized(&self, local: f64) -> f64 {
        if let Some(oracle) = self.state.risk_oracle() {
            if let Some(global) = oracle.realized_today().await {
                return local.min(global);
            }
        }
        local
    }

    /// Open-position count combining the local book with the cluster-wide
    /// view: the LARGER of the two wins (a position opened on another
    /// replica counts against this replica's capacity immediately).
    async fn effective_open_count(&self, module: BotModule, local: usize) -> usize {
        if let Some(oracle) = self.state.risk_oracle() {
            if let Some(global) = oracle.count_open(module).await {
                return local.max(global);
            }
        }
        local
    }

    /// Full entry check. Returns a decision; never panics, never sends anything.
    pub async fn check_entry(&self, req: &EntryRequest) -> RiskDecision {
        let risk = self.risk_config().await;

        let is_sniper = req.module == BotModule::Sniper;
        let is_copy = req.module == BotModule::Copy;
        let is_poly = req.module == BotModule::Polymarket;

        // 1. Kill switch / module / daily loss / module emergency disable ----
        if let Err((code, reason)) = self.preflight_coded(req.module).await {
            return RiskDecision::reject(code, req.requested_quote, reason);
        }

        // 2. Request sanity ------------------------------------------------
        // NaN must reject: matches!-on-partial_cmp makes that explicit.
        if !matches!(
            req.requested_quote.partial_cmp(&0.0),
            Some(std::cmp::Ordering::Greater)
        ) || !req.requested_quote.is_finite()
        {
            return RiskDecision::reject(
                RiskCode::InvalidSize,
                req.requested_quote,
                "requested size must be > 0",
            );
        }

        // 2b. TASK 5 — the ONE global decision (portfolio limits, venue /
        //     strategy kill switches, daily loss, drawdown) over the global
        //     ledger's book. Only an accept reaches the module checks below.
        if let Some(rejected) = self.global_gate(req).await {
            return rejected;
        }

        // 3. Slippage cap --------------------------------------------------
        if req.slippage_bps > risk.max_slippage_bps {
            return RiskDecision::reject(
                RiskCode::SlippageCap,
                req.requested_quote,
                format!(
                    "slippage {}bps exceeds max {}bps",
                    req.slippage_bps, risk.max_slippage_bps
                ),
            );
        }

        // 4. Position count ------------------------------------------------
        //    Cluster-wide when an oracle is attached: the local book only
        //    converges via book sync, so the shared count (max of both) is
        //    what caps global exposure (§Q). The sniper may carry a tighter
        //    concurrent-position cap of its own; the generic one still holds.
        let open = self.state.open_positions_for(req.module).await;
        let open_count = self.effective_open_count(req.module, open.len()).await;
        let position_limit = if is_sniper {
            risk.sniper_position_limit().min(risk.max_open_positions)
        } else if is_copy {
            risk.copy_position_limit().min(risk.max_open_positions)
        } else if is_poly {
            risk.poly_position_limit().min(risk.max_open_positions)
        } else {
            risk.max_open_positions
        };
        if open_count >= position_limit {
            return RiskDecision::reject(
                RiskCode::MaxOpenPositions,
                req.requested_quote,
                format!(
                    "already {} open positions (max {})",
                    open_count, position_limit
                ),
            );
        }

        // 5. Duplicate symbol ---------------------------------------------
        if open.iter().any(|p| p.symbol == req.symbol) {
            return RiskDecision::reject(
                RiskCode::DuplicateSymbol,
                req.requested_quote,
                format!("already holding {}", req.symbol_display),
            );
        }

        // 6. Re-entry cooldown --------------------------------------------
        if !self.state.reentry_allowed(&req.symbol).await {
            let at = self
                .state
                .last_exit(&req.symbol)
                .await
                .map(|t| t.to_rfc3339())
                .unwrap_or_else(|| "unknown".into());
            return RiskDecision::reject(
                RiskCode::ReentryCooldown,
                req.requested_quote,
                format!(
                    "re-entry cooldown active ({}s, last exit {at})",
                    risk.reentry_cooldown_secs
                ),
            );
        }

        // 6b. Sniper-only throttles: per-token attempt cooldown, failed-entry
        //     cooldown and the live-intent cap. Evaluated here (before the
        //     balance read) so a throttled mint never costs an RPC call.
        if is_sniper {
            if let Some(reason) = self.sniper_throttle_reason(req, &risk).await {
                return reason;
            }
        }
        // 6c. Copy-only throttles: failed-entry cooldown and the live-intent
        //     cap, same placement and for the same reason.
        if is_copy {
            if let Some(reason) = self.copy_throttle_reason(req, &risk).await {
                return reason;
            }
        }

        // 7. Balance -------------------------------------------------------
        let reserve = if matches!(req.venue, Venue::PolymarketClob) {
            0.0
        } else {
            risk.min_sol_reserve
        };
        let spendable = req.available_quote - reserve;
        if spendable <= 0.0 {
            return RiskDecision::reject(
                RiskCode::InsufficientBalance,
                req.requested_quote,
                format!(
                    "balance {:.4} below reserve {:.4}",
                    req.available_quote, reserve
                ),
            );
        }

        // 8. Size caps: absolute, fraction of balance, and remaining exposure
        // clamp is NaN-transparent, but TOML config cannot express NaN, so the
        // fraction is always a real number here.
        let fraction_cap = spendable * risk.max_position_fraction.clamp(0.0, 1.0);
        let exposure = self.state.open_exposure(req.module).await;
        let position_cap = if is_sniper {
            risk.sniper_position_cap().min(risk.max_position_quote)
        } else if is_copy {
            risk.copy_position_cap().min(risk.max_position_quote)
        } else if is_poly {
            risk.poly_position_cap().min(risk.max_position_quote)
        } else {
            risk.max_position_quote
        };
        let mut sized = req.requested_quote.min(position_cap).min(fraction_cap);

        // Never let total exposure exceed what the daily loss limit can absorb
        // times a safety factor, so one bad candle cannot blow the account.
        // The sniper's own total-exposure cap can only tighten this envelope.
        let mut exposure_cap =
            (risk.max_position_quote * risk.max_open_positions as f64).max(risk.max_position_quote);
        if is_sniper && risk.sniper_max_total_exposure_quote > 0.0 {
            exposure_cap = exposure_cap.min(risk.sniper_max_total_exposure_quote);
        }
        if is_copy && risk.copy_max_total_exposure_quote > 0.0 {
            exposure_cap = exposure_cap.min(risk.copy_max_total_exposure_quote);
        }
        if is_poly && risk.poly_max_total_exposure_quote > 0.0 {
            exposure_cap = exposure_cap.min(risk.poly_max_total_exposure_quote);
        }
        if exposure + sized > exposure_cap {
            sized = (exposure_cap - exposure).max(0.0);
        }

        if sized > spendable {
            sized = spendable;
        }
        if sized <= 0.0 {
            return RiskDecision::reject(
                RiskCode::ExposureCap,
                req.requested_quote,
                format!(
                    "computed size is zero after caps (exposure {exposure:.4} of cap {exposure_cap:.4}, spendable {spendable:.4})"
                ),
            );
        }

        let verdict = if (sized - req.requested_quote).abs() < 1e-12 {
            RiskVerdict::Allow
        } else {
            RiskVerdict::AllowReduced
        };
        let reason = match verdict {
            RiskVerdict::AllowReduced => format!(
                "size reduced {:.6} -> {:.6} (position_cap={:.4}, fraction_cap={:.4}, exposure_cap={:.4}, spendable={:.4})",
                req.requested_quote, sized, position_cap, fraction_cap, exposure_cap, spendable
            ),
            _ => "ok".into(),
        };

        // 9. Venue-specific checks -----------------------------------------
        if matches!(req.venue, Venue::PolymarketClob) {
            if let Some(price) = req.price {
                if !(price > 0.0 && price < 1.0) {
                    return RiskDecision::reject(
                        RiskCode::VenueCheck,
                        req.requested_quote,
                        format!("price {price} outside (0, 1)"),
                    );
                }
                if price < risk.poly_price_floor || price > risk.poly_price_ceiling {
                    return RiskDecision::reject(
                        RiskCode::VenueCheck,
                        req.requested_quote,
                        format!(
                            "price {price:.4} outside [{:.4}, {:.4}]",
                            risk.poly_price_floor, risk.poly_price_ceiling
                        ),
                    );
                }
            }
            if let Some(liq) = req.liquidity {
                if liq < risk.poly_min_liquidity_usd {
                    return RiskDecision::reject(
                        RiskCode::VenueCheck,
                        req.requested_quote,
                        format!(
                            "liquidity {liq:.0} below minimum {:.0}",
                            risk.poly_min_liquidity_usd
                        ),
                    );
                }
            }
            if let (Some(price), Some(fair)) = (req.price, req.fair_value) {
                let edge = fair - price;
                let min_edge = req
                    .fair_value
                    .map(|_| risk.poly_min_edge)
                    .unwrap_or(risk.poly_min_edge);
                if edge < min_edge {
                    return RiskDecision::reject(
                        RiskCode::VenueCheck,
                        req.requested_quote,
                        format!("edge {edge:.4} below minimum {min_edge:.4} (price {price:.4}, fair {fair:.4})"),
                    );
                }
            }
        }

        // 10. Exit parameters ---------------------------------------------
        // Only meaningful for quoted-price venues (Polymarket). For Solana
        // tokens the caller derives these from the entry price after the fill.
        let stop_loss = req.price.map(|p| p * (1.0 - risk.default_stop_loss_pct));
        let take_profit = req.price.map(|p| p * (1.0 + risk.default_take_profit_pct));

        RiskDecision {
            verdict,
            reason,
            code: None,
            sized_quote: sized,
            requested_quote: req.requested_quote,
            stop_loss: stop_loss.filter(|v| v.is_finite()),
            take_profit: take_profit.filter(|v| v.is_finite()),
            trailing_stop_pct: risk.trailing_stop_pct,
            max_hold_secs: risk.max_hold_secs,
        }
    }

    /// Step 2b of [`RiskEngine::check_entry`]: the global engine's verdict,
    /// mapped onto the module-facing [`RiskCode`]. `None` = accepted.
    async fn global_gate(&self, req: &EntryRequest) -> Option<RiskDecision> {
        let global = self.state.global_risk();
        let request = req.global_request(&self.state).await;
        let ctx = crate::global_risk::DecisionContext {
            global_kill: self.state.kill_switch(),
            marks: self.state.marks().await,
        };
        let decision = global.decide(&request, &ctx).await;
        if decision.accepted() {
            return None;
        }
        let reason = decision
            .reason
            .unwrap_or(crate::global_risk::GlobalRejectReason::InvalidRequest);
        Some(RiskDecision::reject(
            RiskCode::from_global(reason),
            req.requested_quote,
            format!("global risk [{}]: {}", reason, decision.detail),
        ))
    }

    /// Sniper-only throttles (step 6b of [`RiskEngine::check_entry`]).
    /// `None` = not throttled.
    async fn sniper_throttle_reason(
        &self,
        req: &EntryRequest,
        risk: &RiskConfig,
    ) -> Option<RiskDecision> {
        let now = Utc::now();
        if risk.sniper_failed_entry_cooldown_secs > 0 {
            if let Some(at) = self.state.last_failed_entry(&req.symbol).await {
                let age = now.signed_duration_since(at).num_seconds();
                if age < risk.sniper_failed_entry_cooldown_secs {
                    return Some(RiskDecision::reject(
                        RiskCode::SniperFailedEntryCooldown,
                        req.requested_quote,
                        format!(
                            "failed-entry cooldown active ({age}s of {}s since last failed entry)",
                            risk.sniper_failed_entry_cooldown_secs
                        ),
                    ));
                }
            }
        }
        if risk.sniper_token_cooldown_secs > 0 {
            if let Some(at) = self.state.last_entry_attempt(&req.symbol).await {
                let age = now.signed_duration_since(at).num_seconds();
                if age < risk.sniper_token_cooldown_secs {
                    return Some(RiskDecision::reject(
                        RiskCode::SniperTokenCooldown,
                        req.requested_quote,
                        format!(
                            "per-token cooldown active ({age}s of {}s since last attempt)",
                            risk.sniper_token_cooldown_secs
                        ),
                    ));
                }
            }
        }
        if risk.sniper_max_pending_executions > 0 {
            let pending = self.pending_sniper_entries().await;
            if pending >= risk.sniper_max_pending_executions {
                return Some(RiskDecision::reject(
                    RiskCode::SniperPendingCap,
                    req.requested_quote,
                    format!(
                        "{pending} sniper entries still in flight (max {})",
                        risk.sniper_max_pending_executions
                    ),
                ));
            }
        }
        None
    }

    /// Copy-only throttles (`None` = not throttled).
    async fn copy_throttle_reason(
        &self,
        req: &EntryRequest,
        risk: &RiskConfig,
    ) -> Option<RiskDecision> {
        let now = Utc::now();
        if risk.copy_failed_entry_cooldown_secs > 0 {
            if let Some(at) = self.state.last_failed_entry(&req.symbol).await {
                let age = now.signed_duration_since(at).num_seconds();
                if age < risk.copy_failed_entry_cooldown_secs {
                    return Some(RiskDecision::reject(
                        RiskCode::CopyFailedEntryCooldown,
                        req.requested_quote,
                        format!(
                            "failed-entry cooldown active ({age}s of {}s since last failed entry)",
                            risk.copy_failed_entry_cooldown_secs
                        ),
                    ));
                }
            }
        }
        if risk.copy_max_pending_executions > 0 {
            let pending = self.pending_copy_entries().await;
            if pending >= risk.copy_max_pending_executions {
                return Some(RiskDecision::reject(
                    RiskCode::CopyPendingCap,
                    req.requested_quote,
                    format!(
                        "{pending} copy entries still in flight (max {})",
                        risk.copy_max_pending_executions
                    ),
                ));
            }
        }
        None
    }

    /// Static, network-free screening of a freshly launched token.
    ///
    /// The keyword/creator denylists live in the sniper config rather than in
    /// `RiskConfig`, so use [`check_launch_with_lists`] when you have them.
    pub fn check_launch(&self, launch: &TokenLaunch, risk: &RiskConfig) -> Result<(), String> {
        if launch.mint.trim().is_empty() {
            return Err("launch has no mint".into());
        }
        if risk.min_creator_buy_sol > 0.0 && launch.initial_buy_sol < risk.min_creator_buy_sol {
            return Err(format!(
                "creator initial buy {:.4} SOL below minimum {:.4}",
                launch.initial_buy_sol, risk.min_creator_buy_sol
            ));
        }
        if risk.max_launch_market_cap_sol > 0.0
            && launch.market_cap_sol > risk.max_launch_market_cap_sol
        {
            return Err(format!(
                "launch market cap {:.2} SOL above maximum {:.2}",
                launch.market_cap_sol, risk.max_launch_market_cap_sol
            ));
        }
        if risk.min_socials > 0 {
            let count = launch.socials.as_ref().map(|s| s.count()).unwrap_or(0);
            if count < risk.min_socials {
                return Err(format!(
                    "only {count} socials, minimum {}",
                    risk.min_socials
                ));
            }
        }
        Ok(())
    }

    /// Screening that also applies the operator's denylists.
    // The denylists are per-module config slices passed by the caller; a
    // params struct would just hide 4 of the 8 without reducing coupling.
    #[allow(clippy::too_many_arguments)]
    pub fn check_launch_with_lists(
        &self,
        launch: &TokenLaunch,
        risk: &RiskConfig,
        creator_denylist: &[String],
        keyword_denylist: &[String],
        known_bad_creators: &[String],
        now_age_secs: i64,
        max_age_secs: i64,
    ) -> Result<(), String> {
        self.check_launch(launch, risk)?;

        if creator_denylist.iter().any(|c| c == &launch.creator) {
            return Err(format!("creator {} is denylisted", launch.creator));
        }
        if risk.block_repeat_offender_creators
            && known_bad_creators.iter().any(|c| c == &launch.creator)
        {
            return Err(format!(
                "creator {} previously rugged / dumped",
                launch.creator
            ));
        }
        let name = format!("{} {}", launch.name, launch.symbol);
        if let Some(hit) = keyword_hit(&name, keyword_denylist) {
            return Err(format!("name matches denylist keyword '{hit}'"));
        }
        if max_age_secs > 0 && now_age_secs > max_age_secs {
            return Err(format!(
                "launch is {now_age_secs}s old, maximum {max_age_secs}s — too late"
            ));
        }
        Ok(())
    }

    /// Decide whether a whale trade is worth mirroring.
    pub async fn check_copy(&self, trade: &WalletTrade, risk: &RiskConfig) -> Result<f64, String> {
        self.check_copy_coded(trade, risk, 0.0, None)
            .await
            .map(|_| trade.sol_amount)
            .map_err(|(_, reason)| reason)
    }

    /// [`RiskEngine::check_copy`] with the machine-readable code attached and
    /// the leader-level exposure controls (TASK 3): preflight (kill switch,
    /// module, daily loss, `copy_emergency_disable`, copy daily loss), a
    /// positive whale size, the per-leader-per-mint cooldown, staleness, and
    /// — when `requested_quote > 0` — the open exposure and open-position
    /// count already mirrored from this leader against the global
    /// `risk.copy_max_leader_exposure_quote` and the per-wallet caps in
    /// `leader_caps` (`(max_exposure_sol, max_open_positions)`, `0` = off).
    pub async fn check_copy_coded(
        &self,
        trade: &WalletTrade,
        risk: &RiskConfig,
        requested_quote: f64,
        leader_caps: Option<(f64, usize)>,
    ) -> Result<(), (RiskCode, String)> {
        self.preflight_coded(BotModule::Copy).await?;
        if trade.sol_amount <= 0.0 {
            return Err((
                RiskCode::InvalidSize,
                "whale trade has no SOL amount".into(),
            ));
        }
        if !self.state.copy_allowed(&trade.wallet, &trade.mint).await {
            return Err((
                RiskCode::CopyCooldown,
                format!(
                    "copy cooldown active for {} on {}",
                    trade.wallet, trade.mint
                ),
            ));
        }
        let staleness = Utc::now()
            .signed_duration_since(trade.observed_at)
            .num_seconds();
        if staleness > risk.copy_cooldown_secs.max(30) {
            return Err((
                RiskCode::StaleSignal,
                format!("whale trade is {staleness}s stale"),
            ));
        }
        if requested_quote > 0.0 {
            let (leader_exposure, leader_open) = self.leader_exposure(&trade.wallet).await;
            let (rule_cap, rule_open_cap) = leader_caps.unwrap_or((0.0, 0));
            let mut cap = risk.copy_max_leader_exposure_quote;
            if rule_cap > 0.0 {
                cap = if cap > 0.0 {
                    cap.min(rule_cap)
                } else {
                    rule_cap
                };
            }
            if cap > 0.0 && leader_exposure + requested_quote > cap {
                return Err((
                    RiskCode::CopyLeaderExposure,
                    format!(
                        "leader exposure {:.4} + {:.4} would exceed cap {:.4}",
                        leader_exposure, requested_quote, cap
                    ),
                ));
            }
            if rule_open_cap > 0 && leader_open >= rule_open_cap {
                return Err((
                    RiskCode::CopyLeaderExposure,
                    format!(
                        "already {leader_open} open positions mirrored from this leader (max {rule_open_cap})"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Polymarket-scoped order-book controls (TASK 4). The ONE coded
    /// pre-check the Polymarket pipeline runs before [`RiskEngine::check_entry`]
    /// sizes the entry: preflight (kill switch, module, daily loss,
    /// `poly_emergency_disable`, Polymarket daily loss), the resting-order cap,
    /// the total-exposure envelope INCLUDING resting orders (positions alone
    /// are what `check_entry` sees — unfilled buys still commit collateral)
    /// and the per-market exposure cap.
    ///
    /// * `requested_quote` — USDC the decision wants to commit;
    /// * `open_orders` — resting (non-terminal) CLOB orders right now;
    /// * `resting_quote` — USDC committed by those resting orders in total;
    /// * `market_quote` — USDC already committed inside this market
    ///   (open positions + resting orders).
    pub async fn check_polymarket_coded(
        &self,
        risk: &RiskConfig,
        requested_quote: f64,
        open_orders: usize,
        resting_quote: f64,
        market_quote: f64,
    ) -> Result<(), (RiskCode, String)> {
        self.preflight_coded(BotModule::Polymarket).await?;
        if !requested_quote.is_finite() || requested_quote <= 0.0 {
            return Err((
                RiskCode::InvalidSize,
                "polymarket order has no positive notional".into(),
            ));
        }
        if risk.poly_max_open_orders > 0 && open_orders >= risk.poly_max_open_orders {
            return Err((
                RiskCode::PolyOpenOrderCap,
                format!(
                    "{open_orders} resting polymarket orders (max {})",
                    risk.poly_max_open_orders
                ),
            ));
        }
        if risk.poly_max_total_exposure_quote > 0.0 {
            let positions = self.state.open_exposure(BotModule::Polymarket).await;
            let total = positions + resting_quote.max(0.0);
            if total + requested_quote > risk.poly_max_total_exposure_quote {
                return Err((
                    RiskCode::ExposureCap,
                    format!(
                        "polymarket exposure {:.4} (positions {:.4} + resting {:.4}) + {:.4} would exceed cap {:.4}",
                        total, positions, resting_quote.max(0.0), requested_quote,
                        risk.poly_max_total_exposure_quote
                    ),
                ));
            }
        }
        if risk.poly_max_market_exposure_quote > 0.0
            && market_quote.max(0.0) + requested_quote > risk.poly_max_market_exposure_quote
        {
            return Err((
                RiskCode::PolyMarketExposure,
                format!(
                    "market exposure {:.4} + {:.4} would exceed per-market cap {:.4}",
                    market_quote.max(0.0),
                    requested_quote,
                    risk.poly_max_market_exposure_quote
                ),
            ));
        }
        Ok(())
    }

    /// Open copy exposure (cost basis or notional, whichever is larger) and
    /// open-position count mirrored from `leader`.
    pub async fn leader_exposure(&self, leader: &str) -> (f64, usize) {
        let open = self.state.open_positions_for(BotModule::Copy).await;
        let mine: Vec<&Position> = open
            .iter()
            .filter(|p| p.copied_wallet.as_deref() == Some(leader))
            .collect();
        let exposure = mine.iter().map(|p| p.cost_basis.max(p.notional())).sum();
        (exposure, mine.len())
    }

    /// Evaluate all exit rules against a live mark price.
    pub async fn check_exit(&self, position: &Position, mark: f64) -> ExitDecision {
        let risk = self.risk_config().await;

        if self.state.kill_switch() {
            return ExitDecision::exit(
                ExitRule::KillSwitch,
                1.0,
                "kill switch engaged — flattening position".to_string(),
            );
        }
        if position.qty <= 0.0 {
            return ExitDecision::hold();
        }
        if !(mark.is_finite() && mark > 0.0) {
            return ExitDecision::hold();
        }

        // Stop loss (absolute price or derived from entry).
        let stop = position
            .stop_loss
            .unwrap_or(position.avg_entry * (1.0 - risk.default_stop_loss_pct));
        if stop > 0.0 && mark <= stop {
            return ExitDecision::exit(
                ExitRule::StopLoss,
                1.0,
                format!(
                    "mark {mark:.8} <= stop {stop:.8} (entry {:.8}, {:.1}%)",
                    position.avg_entry,
                    (mark / position.avg_entry - 1.0) * 100.0
                ),
            );
        }

        // Take profit.
        let tp = position
            .take_profit
            .unwrap_or(position.avg_entry * (1.0 + risk.default_take_profit_pct));
        if tp > 0.0 && mark >= tp {
            return ExitDecision::exit(
                ExitRule::TakeProfit,
                1.0,
                format!(
                    "mark {mark:.8} >= take profit {tp:.8} (+{:.1}%)",
                    (mark / position.avg_entry - 1.0) * 100.0
                ),
            );
        }

        // Trailing stop from the high-water mark.
        if let Some(trail_pct) = position.trailing_stop.or(risk.trailing_stop_pct) {
            let hwm = position
                .trailing_high_water
                .unwrap_or(position.avg_entry)
                .max(mark);
            let trail = hwm * (1.0 - trail_pct);
            // Only trail once we are in profit, otherwise the stop loss owns it.
            if hwm > position.avg_entry && mark <= trail {
                return ExitDecision::exit(
                    ExitRule::TrailingStop,
                    1.0,
                    format!(
                        "mark {mark:.8} <= trailing stop {trail:.8} (hwm {hwm:.8}, trail {:.0}%)",
                        trail_pct * 100.0
                    ),
                );
            }
        }

        // Time stop. `None` disables the rule; any non-positive value means
        // "already expired", so the position is closed on the next sweep.
        if let Some(max_hold) = position.max_hold_secs.or(risk.max_hold_secs) {
            let age = position.age_secs();
            if age >= max_hold {
                return ExitDecision::exit(
                    ExitRule::MaxHoldTime,
                    1.0,
                    format!("held {age}s >= max {max_hold}s"),
                );
            }
        }

        ExitDecision::hold()
    }

    /// Record realised PnL and trip the daily loss limit if it is breached.
    pub async fn book_pnl(&self, module: BotModule, pnl: f64) {
        self.state.add_realized(module, pnl).await;
        let risk = self.risk_config().await;
        if risk.daily_loss_limit_quote <= 0.0 {
            return;
        }
        let daily = self.state.daily_stats().await;
        let realized = self.effective_realized(daily.realized_pnl).await;
        if realized <= -risk.daily_loss_limit_quote && !daily.loss_limit_tripped {
            let first_time = self.state.mark_loss_limit_tripped().await;
            if first_time {
                self.state
                    .events
                    .publish(crate::events::AppEvent::RiskRejected {
                        ts: Utc::now(),
                        module,
                        symbol: "*".into(),
                        reason: format!(
                            "DAILY LOSS LIMIT HIT: realized {:.4} <= -{:.4}. Trading disabled until the next UTC day.",
                            realized, risk.daily_loss_limit_quote
                        ),
                    });
                // Disable the trading modules so nothing new is opened.
                for m in BotModule::TRADING {
                    self.state.set_enabled(m, false).await;
                }
            }
        }
    }

    /// Sweep open positions and close whatever an exit rule now demands.
    /// Returns the positions that need to be sold so the caller can execute.
    pub async fn positions_to_close(&self, module: BotModule) -> Vec<(Position, ExitDecision)> {
        let mut out = Vec::new();
        for position in self.state.open_positions_for(module).await {
            let mark = if position.last_mark > 0.0 {
                position.last_mark
            } else {
                position.avg_entry
            };
            let decision = self.check_exit(&position, mark).await;
            if decision.should_exit {
                out.push((position, decision));
            }
        }
        out
    }
}

/// Case-insensitive substring match against a denylist.
fn keyword_hit(haystack: &str, words: &[String]) -> Option<String> {
    let h = haystack.to_lowercase();
    words
        .iter()
        .map(|w| w.trim().to_lowercase())
        .filter(|w| !w.is_empty())
        .find(|w| h.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::models::{PositionSide, TradeSource};

    /// Deterministic cluster-wide view for oracle tests.
    struct FakeOracle {
        open: Option<usize>,
        pnl: Option<f64>,
    }

    #[async_trait]
    impl GlobalRiskOracle for FakeOracle {
        async fn count_open(&self, _module: BotModule) -> Option<usize> {
            self.open
        }
        async fn realized_today(&self) -> Option<f64> {
            self.pnl
        }
    }

    /// §Q gap closure: the cluster-wide oracle can only TIGHTEN limits —
    /// capacity from the shared count, daily loss from the more negative
    /// view; `None` (store unknown) falls back to the local view (§K).
    #[tokio::test]
    async fn global_oracle_tightens_never_loosens() {
        // Capacity: local book empty, cluster at the default max (8) → reject.
        let e = engine();
        e.state().attach_risk_oracle(Arc::new(FakeOracle {
            open: Some(8),
            pnl: None,
        }));
        let d = e.check_entry(&request(0.01, 10.0)).await;
        assert!(
            !d.allowed() && d.reason.contains("open positions"),
            "{}",
            d.reason
        );

        // Daily loss: local accumulator zero, cluster beyond the -2.0 default
        // limit → reject.
        let e = engine();
        e.state().attach_risk_oracle(Arc::new(FakeOracle {
            open: None,
            pnl: Some(-50.0),
        }));
        let d = e.check_entry(&request(0.01, 10.0)).await;
        assert!(
            !d.allowed() && d.reason.contains("daily realized loss"),
            "{}",
            d.reason
        );

        // The same cluster loss trips the latch through book_pnl and disables
        // the trading modules (which now propagates cluster-wide via flags).
        e.book_pnl(BotModule::Sniper, 0.0).await;
        assert!(!e.state().is_enabled(BotModule::Sniper).await);

        // Unknown (None on both) → purely local view → allowed.
        let e = engine();
        e.state().attach_risk_oracle(Arc::new(FakeOracle {
            open: None,
            pnl: None,
        }));
        assert!(e.check_entry(&request(0.01, 10.0)).await.allowed());

        // No oracle attached at all → legacy local behaviour.
        let e = engine();
        assert!(e.check_entry(&request(0.01, 10.0)).await.allowed());
    }

    fn engine() -> RiskEngine {
        let mut cfg = AppConfig::from_defaults();
        cfg.raw.sniper.enabled = true;
        cfg.raw.copy.enabled = true;
        cfg.raw.polymarket.enabled = true;
        RiskEngine::new(crate::state::AppState::new(cfg))
    }

    fn request(quote: f64, available: f64) -> EntryRequest {
        EntryRequest {
            module: BotModule::Sniper,
            venue: Venue::PumpFun,
            symbol: "So1111test".into(),
            symbol_display: "TEST".into(),
            requested_quote: quote,
            available_quote: available,
            slippage_bps: 1500,
            price: None,
            fair_value: None,
            liquidity: None,
            wallet: String::new(),
            strategy: String::new(),
        }
    }

    #[tokio::test]
    async fn kill_switch_blocks_everything() {
        let e = engine();
        e.state().set_kill_switch(true, "test").await;
        let d = e.check_entry(&request(0.01, 10.0)).await;
        assert!(!d.allowed());
        assert!(d.reason.contains("kill switch"), "{}", d.reason);
    }

    #[tokio::test]
    async fn disabled_module_is_rejected() {
        let e = engine();
        e.state().set_enabled(BotModule::Sniper, false).await;
        let d = e.check_entry(&request(0.01, 10.0)).await;
        assert!(!d.allowed());
        assert!(d.reason.contains("disabled"), "{}", d.reason);
    }

    #[tokio::test]
    async fn size_is_capped_by_max_position_quote() {
        let e = engine();
        // Default max_position_quote is 0.5 SOL.
        let d = e.check_entry(&request(10.0, 100.0)).await;
        assert!(d.allowed());
        assert!(d.sized_quote <= 0.5 + 1e-9, "sized = {}", d.sized_quote);
        assert_eq!(d.verdict, RiskVerdict::AllowReduced);
    }

    #[tokio::test]
    async fn insufficient_balance_is_rejected() {
        let e = engine();
        // min_sol_reserve defaults to 0.05, so 0.04 spendable is nothing.
        let d = e.check_entry(&request(0.01, 0.04)).await;
        assert!(!d.allowed());
        assert!(d.reason.contains("reserve"), "{}", d.reason);
    }

    #[tokio::test]
    async fn slippage_above_cap_is_rejected() {
        let e = engine();
        let mut r = request(0.01, 10.0);
        r.slippage_bps = 9_999;
        let d = e.check_entry(&r).await;
        assert!(!d.allowed());
        assert!(d.reason.contains("slippage"), "{}", d.reason);
    }

    #[tokio::test]
    async fn polymarket_edge_gate_works() {
        let e = engine();
        let mut r = request(5.0, 1_000.0);
        r.module = BotModule::Polymarket;
        r.venue = Venue::PolymarketClob;
        r.price = Some(0.50);
        r.fair_value = Some(0.51); // edge 0.01 < default min_edge 0.03
        r.liquidity = Some(50_000.0);
        let d = e.check_entry(&r).await;
        assert!(!d.allowed());
        assert!(d.reason.contains("edge"), "{}", d.reason);

        r.fair_value = Some(0.60); // edge 0.10 > 0.03
        let d = e.check_entry(&r).await;
        assert!(d.allowed(), "{}", d.reason);
    }

    #[tokio::test]
    async fn stop_loss_and_take_profit_fire() {
        let e = engine();
        let mut p = Position::new(
            "p-test".into(),
            TradeSource::Sniper,
            Venue::PumpFun,
            crate::models::ExecutionMode::Paper,
            "mint".into(),
            "TEST".into(),
            "SOL".into(),
        );
        p.qty = 1000.0;
        p.avg_entry = 0.001;
        p.cost_basis = 1.0;
        p.last_mark = 0.001;

        let d = e.check_exit(&p, 0.0006).await; // -40%
        assert!(d.should_exit);
        assert_eq!(d.rule, Some(ExitRule::StopLoss));

        let d = e.check_exit(&p, 0.0021).await; // +110%
        assert!(d.should_exit);
        assert_eq!(d.rule, Some(ExitRule::TakeProfit));

        let d = e.check_exit(&p, 0.0011).await; // +10%, inside both bands
        assert!(!d.should_exit, "{}", d.reason);
    }

    #[tokio::test]
    async fn trailing_stop_only_fires_in_profit() {
        let e = engine();
        let mut p = Position::new(
            "p-trail".into(),
            TradeSource::Sniper,
            Venue::PumpFun,
            crate::models::ExecutionMode::Paper,
            "mint".into(),
            "TRAIL".into(),
            "SOL".into(),
        );
        p.qty = 1.0;
        p.avg_entry = 1.0;
        p.cost_basis = 1.0;
        p.trailing_stop = Some(0.25);
        p.trailing_high_water = Some(2.0);

        // Ran up to 2.0, now 30% below that high water mark => trail fires.
        let d = e.check_exit(&p, 1.4).await;
        assert!(d.should_exit);
        assert_eq!(d.rule, Some(ExitRule::TrailingStop));

        // Never got above entry: hwm <= avg_entry, so the trailing rule must
        // stay silent and leave the decision to the stop loss.
        p.trailing_high_water = Some(1.0);
        p.stop_loss = Some(0.50);
        let d = e.check_exit(&p, 0.85).await;
        assert!(!d.should_exit, "{}", d.reason);

        // ... and once the stop loss level is breached, it is the stop that fires.
        let d = e.check_exit(&p, 0.45).await;
        assert!(d.should_exit);
        assert_eq!(d.rule, Some(ExitRule::StopLoss));
    }

    #[tokio::test]
    async fn max_hold_time_fires() {
        let e = engine();
        let mut p = Position::new(
            "p-time".into(),
            TradeSource::Sniper,
            Venue::PumpFun,
            crate::models::ExecutionMode::Paper,
            "mint".into(),
            "TIME".into(),
            "SOL".into(),
        );
        p.qty = 1.0;
        p.avg_entry = 1.0;
        p.last_mark = 1.0;
        // Positions inherit their exit parameters from the RiskDecision that
        // authorised the entry; the test sets them by hand for the same effect.
        p.stop_loss = Some(0.50);
        p.take_profit = Some(3.00);
        p.trailing_stop = None;
        p.max_hold_secs = Some(-1); // already expired
        let d = e.check_exit(&p, 1.0).await;
        assert!(d.should_exit);
        assert_eq!(d.rule, Some(ExitRule::MaxHoldTime));
    }

    #[tokio::test]
    async fn duplicate_symbol_is_rejected() {
        let e = engine();
        let mut p = Position::new(
            "p-dup".into(),
            TradeSource::Sniper,
            Venue::PumpFun,
            crate::models::ExecutionMode::Paper,
            "So1111test".into(),
            "TEST".into(),
            "SOL".into(),
        );
        p.qty = 1.0;
        p.avg_entry = 1.0;
        e.state().upsert_position(p).await;

        let d = e.check_entry(&request(0.01, 10.0)).await;
        assert!(!d.allowed());
        assert!(d.reason.contains("already holding"), "{}", d.reason);
        let _ = PositionSide::Long;
    }

    // ------------------------------------------------------------------
    // Sniper exposure controls (TASK 2 §I) — all evaluated by THIS engine
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn rejections_carry_machine_readable_codes_and_allows_do_not() {
        let e = engine();
        let ok = e.check_entry(&request(0.01, 10.0)).await;
        assert!(ok.allowed());
        assert_eq!(ok.code, None);

        e.state().set_kill_switch(true, "test").await;
        let d = e.check_entry(&request(0.01, 10.0)).await;
        assert_eq!(d.code, Some(RiskCode::KillSwitch));
        assert_eq!(d.code.unwrap().as_str(), "kill_switch");
        e.state().set_kill_switch(false, "test").await;

        let mut r = request(0.01, 10.0);
        r.slippage_bps = 9_999;
        assert_eq!(e.check_entry(&r).await.code, Some(RiskCode::SlippageCap));
        let d = e.check_entry(&request(f64::NAN, 10.0)).await;
        assert_eq!(d.code, Some(RiskCode::InvalidSize));
        let d = e.check_entry(&request(0.01, 0.01)).await;
        assert_eq!(d.code, Some(RiskCode::InsufficientBalance));
        assert!(RiskCode::ExposureCap.is_exposure_limit());
        assert!(!RiskCode::ReentryCooldown.is_exposure_limit());
    }

    #[tokio::test]
    async fn sniper_emergency_disable_refuses_entries_but_not_other_modules() {
        let e = engine();
        e.state()
            .update_config(|c| c.risk.sniper_emergency_disable = true)
            .await;
        let d = e.check_entry(&request(0.01, 10.0)).await;
        assert!(!d.allowed());
        assert_eq!(d.code, Some(RiskCode::SniperEmergencyDisabled));
        assert!(e.preflight(BotModule::Sniper).await.is_err());
        // Copy trading is untouched by the sniper-only latch.
        let mut r = request(0.01, 10.0);
        r.module = BotModule::Copy;
        assert!(e.check_entry(&r).await.allowed());
        assert!(e.preflight(BotModule::Copy).await.is_ok());
    }

    #[tokio::test]
    async fn sniper_daily_loss_limit_is_module_scoped() {
        let e = engine();
        e.state()
            .update_config(|c| c.risk.sniper_daily_loss_limit_quote = 0.5)
            .await;
        // A copy-module loss does not count against the sniper cap.
        e.state().add_realized(BotModule::Copy, -0.6).await;
        assert!(e.check_entry(&request(0.01, 10.0)).await.allowed());
        // A sniper loss at the cap trips it (the generic 2.0 limit is not hit).
        e.state().add_realized(BotModule::Sniper, -0.5).await;
        let d = e.check_entry(&request(0.01, 10.0)).await;
        assert_eq!(d.code, Some(RiskCode::SniperDailyLoss), "{}", d.reason);
        assert_eq!(e.state().daily_realized(BotModule::Sniper).await, -0.5);
        assert_eq!(e.state().daily_realized(BotModule::Copy).await, -0.6);
    }

    #[tokio::test]
    async fn sniper_cooldowns_gate_by_mint_and_by_outcome() {
        let e = engine();
        e.state()
            .update_config(|c| {
                c.risk.sniper_token_cooldown_secs = 60;
                c.risk.sniper_failed_entry_cooldown_secs = 120;
            })
            .await;
        // Fresh mint: nothing recorded → allowed.
        assert!(e.check_entry(&request(0.01, 10.0)).await.allowed());
        // An attempt was made a moment ago → per-token cooldown.
        e.state().note_entry_attempt("So1111test").await;
        let d = e.check_entry(&request(0.01, 10.0)).await;
        assert_eq!(d.code, Some(RiskCode::SniperTokenCooldown), "{}", d.reason);
        // A different mint is unaffected.
        let mut other = request(0.01, 10.0);
        other.symbol = "So1111other".into();
        assert!(e.check_entry(&other).await.allowed());
        // A failed entry wins over the attempt cooldown (more specific).
        e.state().note_failed_entry("So1111test").await;
        let d = e.check_entry(&request(0.01, 10.0)).await;
        assert_eq!(d.code, Some(RiskCode::SniperFailedEntryCooldown));
        // Both knobs at zero = off, even with the timestamps present.
        e.state()
            .update_config(|c| {
                c.risk.sniper_token_cooldown_secs = 0;
                c.risk.sniper_failed_entry_cooldown_secs = 0;
            })
            .await;
        assert!(e.check_entry(&request(0.01, 10.0)).await.allowed());
    }

    #[tokio::test]
    async fn sniper_position_and_exposure_caps_only_tighten() {
        let e = engine();
        e.state()
            .update_config(|c| {
                c.risk.max_open_positions = 8;
                c.risk.sniper_max_concurrent_positions = 1;
                c.risk.max_position_quote = 0.5;
                c.risk.sniper_max_position_quote = 0.05;
                c.risk.sniper_max_total_exposure_quote = 0.08;
                c.risk.max_position_fraction = 1.0;
            })
            .await;
        // Per-token cap: 0.2 requested → 0.05 (sniper cap < generic cap).
        let d = e.check_entry(&request(0.2, 10.0)).await;
        assert!(d.allowed());
        assert_eq!(d.verdict, RiskVerdict::AllowReduced);
        assert!((d.sized_quote - 0.05).abs() < 1e-12, "{}", d.sized_quote);

        // Total exposure: 0.06 already open → only 0.02 of the 0.08 left.
        let mut p = Position::new(
            "p-open".into(),
            TradeSource::Sniper,
            Venue::PumpFun,
            crate::models::ExecutionMode::Paper,
            "So1111held".into(),
            "HELD".into(),
            "SOL".into(),
        );
        p.qty = 1.0;
        p.avg_entry = 0.06;
        p.cost_basis = 0.06;
        p.last_mark = 0.06;
        e.state().upsert_position(p).await;
        // ...but the concurrent-position cap (1) fires first.
        let d = e.check_entry(&request(0.05, 10.0)).await;
        assert_eq!(d.code, Some(RiskCode::MaxOpenPositions), "{}", d.reason);
        e.state()
            .update_config(|c| c.risk.sniper_max_concurrent_positions = 0)
            .await;
        let d = e.check_entry(&request(0.05, 10.0)).await;
        assert!(d.allowed(), "{}", d.reason);
        assert!((d.sized_quote - 0.02).abs() < 1e-9, "{}", d.sized_quote);

        // Copy trading keeps the generic caps.
        let mut r = request(0.2, 10.0);
        r.module = BotModule::Copy;
        let d = e.check_entry(&r).await;
        assert!((d.sized_quote - 0.2).abs() < 1e-12, "{}", d.sized_quote);
    }

    #[tokio::test]
    async fn sniper_pending_execution_cap_reads_the_execution_ledger() {
        let e = engine();
        e.state()
            .update_config(|c| c.risk.sniper_max_pending_executions = 1)
            .await;
        let ledger = crate::execution::ledger();
        let id = format!(
            "int_test_pending_{}",
            crate::execution::digest_hex(b"risk-pending")
        );
        ledger
            .begin(crate::execution::ExecutionIntent {
                intent_id: id.clone(),
                module: "sniper".into(),
                label: "snipe-TEST".into(),
                wallet: "w".into(),
                symbol: "So1111test".into(),
            })
            .await
            .unwrap();
        assert!(e.pending_sniper_entries().await >= 1);
        let d = e.check_entry(&request(0.01, 10.0)).await;
        assert_eq!(d.code, Some(RiskCode::SniperPendingCap), "{}", d.reason);
        // Settling the intent frees the slot; exits (`exit-…`) never count.
        ledger
            .fail(
                &id,
                crate::execution::FailureClass::Rejected,
                "test settled",
            )
            .await
            .unwrap();
        let exit_id = format!(
            "int_test_exit_{}",
            crate::execution::digest_hex(b"risk-exit")
        );
        ledger
            .begin(crate::execution::ExecutionIntent {
                intent_id: exit_id.clone(),
                module: "sniper".into(),
                label: "exit-So1111test".into(),
                wallet: "w".into(),
                symbol: "So1111test".into(),
            })
            .await
            .unwrap();
        let d = e.check_entry(&request(0.01, 10.0)).await;
        assert_ne!(d.code, Some(RiskCode::SniperPendingCap), "{}", d.reason);
        ledger
            .fail(
                &exit_id,
                crate::execution::FailureClass::Rejected,
                "test settled",
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn forced_exit_decision_uses_the_stale_rule() {
        let d = ExitDecision::forced(ExitRule::StalePosition, "no mark for 900s");
        assert!(d.should_exit);
        assert_eq!(d.rule, Some(ExitRule::StalePosition));
        assert_eq!(d.fraction, 1.0);
        assert_eq!(ExitRule::StalePosition.as_str(), "stale_position");
    }

    // ------------------------------------------------------------------
    // Copy-trading controls (TASK 3)
    // ------------------------------------------------------------------

    fn whale_trade(wallet: &str, mint: &str, sol: f64) -> WalletTrade {
        WalletTrade {
            wallet: wallet.into(),
            signature: "sig".into(),
            slot: 1,
            block_time: None,
            side: PositionSide::Long,
            mint: mint.into(),
            symbol: None,
            token_amount: 1.0,
            sol_amount: sol,
            venue: Venue::PumpFun,
            fee_sol: 0.0,
            discriminator: None,
            observed_at: Utc::now(),
        }
    }

    fn copy_position(id: &str, mint: &str, leader: &str, cost: f64) -> Position {
        let mut p = Position::new(
            id.into(),
            TradeSource::Copy,
            Venue::PumpFun,
            crate::models::ExecutionMode::Paper,
            mint.into(),
            mint.into(),
            "SOL".into(),
        );
        p.apply_buy(100.0, cost / 100.0, cost);
        p.copied_wallet = Some(leader.into());
        p
    }

    #[tokio::test]
    async fn copy_emergency_disable_refuses_copy_entries_only() {
        let e = engine();
        e.state()
            .update_config(|c| c.risk.copy_emergency_disable = true)
            .await;
        let mut r = request(0.01, 10.0);
        r.module = BotModule::Copy;
        let d = e.check_entry(&r).await;
        assert_eq!(
            d.code,
            Some(RiskCode::CopyEmergencyDisabled),
            "{}",
            d.reason
        );
        let err = e.preflight_coded(BotModule::Copy).await.unwrap_err();
        assert_eq!(err.0, RiskCode::CopyEmergencyDisabled);
        assert_eq!(err.0.as_str(), "copy_emergency_disabled");
        // Sniper untouched.
        assert!(e.check_entry(&request(0.01, 10.0)).await.allowed());
        let risk = e.state().config_snapshot().await.risk;
        let coded = e
            .check_copy_coded(&whale_trade("w", "m", 1.0), &risk, 0.05, None)
            .await;
        assert_eq!(coded.unwrap_err().0, RiskCode::CopyEmergencyDisabled);
    }

    #[tokio::test]
    async fn copy_daily_loss_limit_is_module_scoped() {
        let e = engine();
        e.state()
            .update_config(|c| c.risk.copy_daily_loss_limit_quote = 0.5)
            .await;
        e.state().add_realized(BotModule::Sniper, -0.6).await;
        let mut r = request(0.01, 10.0);
        r.module = BotModule::Copy;
        assert!(e.check_entry(&r).await.allowed());
        e.state().add_realized(BotModule::Copy, -0.5).await;
        let d = e.check_entry(&r).await;
        assert_eq!(d.code, Some(RiskCode::CopyDailyLoss), "{}", d.reason);
        assert!(RiskCode::CopyDailyLoss.is_exposure_limit());
    }

    #[tokio::test]
    async fn copy_position_caps_are_module_scoped() {
        let e = engine();
        e.state()
            .update_config(|c| {
                c.risk.max_position_quote = 0.5;
                c.risk.copy_max_position_quote = 0.2;
                c.risk.copy_max_concurrent_positions = 1;
                c.risk.copy_max_total_exposure_quote = 0.3;
            })
            .await;
        let mut r = request(0.4, 10.0);
        r.module = BotModule::Copy;
        let d = e.check_entry(&r).await;
        assert_eq!(d.verdict, RiskVerdict::AllowReduced, "{}", d.reason);
        assert!((d.sized_quote - 0.2).abs() < 1e-9);
        // The sniper keeps the generic cap.
        let d = e.check_entry(&request(0.4, 10.0)).await;
        assert!((d.sized_quote - 0.4).abs() < 1e-9);
        // One open copy position → the copy limit (1) is reached.
        e.state()
            .upsert_position(copy_position("p1", "other", "w", 0.1))
            .await;
        let d = e.check_entry(&r).await;
        assert_eq!(d.code, Some(RiskCode::MaxOpenPositions), "{}", d.reason);
        assert_eq!(
            e.state().config_snapshot().await.risk.copy_position_limit(),
            1
        );
        assert!((e.state().config_snapshot().await.risk.copy_position_cap() - 0.2).abs() < 1e-9);
    }

    #[tokio::test]
    async fn copy_leader_exposure_and_cooldown_are_coded() {
        let e = engine();
        e.state()
            .update_config(|c| {
                c.risk.copy_max_leader_exposure_quote = 0.25;
                c.risk.copy_cooldown_secs = 120;
            })
            .await;
        let risk = e.state().config_snapshot().await.risk;
        e.state()
            .upsert_position(copy_position("p1", "m1", "whale", 0.2))
            .await;
        e.state()
            .upsert_position(copy_position("p2", "m2", "other", 0.9))
            .await;
        let (exposure, open) = e.leader_exposure("whale").await;
        assert!((exposure - 0.2).abs() < 1e-9);
        assert_eq!(open, 1);
        // 0.2 + 0.1 > 0.25 → global leader cap.
        let err = e
            .check_copy_coded(&whale_trade("whale", "m3", 1.0), &risk, 0.1, None)
            .await
            .unwrap_err();
        assert_eq!(err.0, RiskCode::CopyLeaderExposure);
        assert!(err.0.is_exposure_limit());
        // Under the cap → ok; the per-rule cap can only tighten.
        assert!(e
            .check_copy_coded(&whale_trade("whale", "m3", 1.0), &risk, 0.04, None)
            .await
            .is_ok());
        let err = e
            .check_copy_coded(
                &whale_trade("whale", "m3", 1.0),
                &risk,
                0.04,
                Some((0.21, 0)),
            )
            .await
            .unwrap_err();
        assert_eq!(err.0, RiskCode::CopyLeaderExposure);
        // Per-rule open-position cap.
        let err = e
            .check_copy_coded(
                &whale_trade("whale", "m3", 1.0),
                &risk,
                0.01,
                Some((0.0, 1)),
            )
            .await
            .unwrap_err();
        assert!(err.1.contains("open positions"), "{}", err.1);
        // requested_quote = 0 skips the exposure checks (legacy check_copy).
        assert_eq!(
            e.check_copy(&whale_trade("whale", "m3", 1.0), &risk).await,
            Ok(1.0)
        );
        // Cooldown after a copy of the same leader+mint.
        e.state().mark_copied("whale", "m3").await;
        let err = e
            .check_copy_coded(&whale_trade("whale", "m3", 1.0), &risk, 0.01, None)
            .await
            .unwrap_err();
        assert_eq!(err.0, RiskCode::CopyCooldown);
        // Stale and empty trades are coded too.
        let mut stale = whale_trade("whale", "m4", 1.0);
        stale.observed_at = Utc::now() - chrono::Duration::seconds(500);
        assert_eq!(
            e.check_copy_coded(&stale, &risk, 0.01, None)
                .await
                .unwrap_err()
                .0,
            RiskCode::StaleSignal
        );
        assert_eq!(
            e.check_copy_coded(&whale_trade("whale", "m4", 0.0), &risk, 0.01, None)
                .await
                .unwrap_err()
                .0,
            RiskCode::InvalidSize
        );
    }

    #[tokio::test]
    async fn copy_pending_cap_and_failed_entry_cooldown_throttle_entries() {
        let e = engine();
        e.state()
            .update_config(|c| {
                c.risk.copy_max_pending_executions = 1;
                c.risk.copy_failed_entry_cooldown_secs = 60;
            })
            .await;
        let mut r = request(0.01, 10.0);
        r.module = BotModule::Copy;
        r.symbol = "CopyThrottleMint".into();
        assert!(e.check_entry(&r).await.allowed());
        e.state().note_failed_entry("CopyThrottleMint").await;
        let d = e.check_entry(&r).await;
        assert_eq!(
            d.code,
            Some(RiskCode::CopyFailedEntryCooldown),
            "{}",
            d.reason
        );
        assert_eq!(
            RiskCode::CopyFailedEntryCooldown.as_str(),
            "copy_failed_entry_cooldown"
        );
        // A live copy ENTRY intent hits the pending cap; exits never count.
        let mut r2 = r.clone();
        r2.symbol = "CopyThrottleMint2".into();
        crate::execution::ledger()
            .begin(crate::execution::ExecutionIntent {
                intent_id: "int_risk_copy_pending_exit".into(),
                module: "copy".into(),
                label: "copy-exit-x".into(),
                wallet: "w".into(),
                symbol: "x".into(),
            })
            .await
            .unwrap();
        assert!(e.check_entry(&r2).await.allowed());
        assert_eq!(e.pending_copy_entries().await, 0);
        crate::execution::ledger()
            .begin(crate::execution::ExecutionIntent {
                intent_id: "int_risk_copy_pending_entry".into(),
                module: "copy".into(),
                label: "copy-jup-x".into(),
                wallet: "w".into(),
                symbol: "x".into(),
            })
            .await
            .unwrap();
        assert!(e.pending_copy_entries().await >= 1);
        let d = e.check_entry(&r2).await;
        assert_eq!(d.code, Some(RiskCode::CopyPendingCap), "{}", d.reason);
        crate::execution::ledger()
            .fail(
                "int_risk_copy_pending_entry",
                crate::execution::FailureClass::BlockhashExpired,
                "test",
            )
            .await
            .unwrap();
        crate::execution::ledger()
            .fail(
                "int_risk_copy_pending_exit",
                crate::execution::FailureClass::BlockhashExpired,
                "test",
            )
            .await
            .unwrap();
    }

    // ------------------------------------------------------------------
    // Polymarket-scoped controls (TASK 4)
    // ------------------------------------------------------------------

    fn poly_request(quote: f64, available: f64) -> EntryRequest {
        EntryRequest {
            module: BotModule::Polymarket,
            venue: Venue::PolymarketClob,
            symbol: "123456789".into(),
            symbol_display: "Mock market Yes".into(),
            requested_quote: quote,
            available_quote: available,
            slippage_bps: 0,
            price: Some(0.4),
            fair_value: None,
            liquidity: Some(1000.0),
            wallet: String::new(),
            strategy: String::new(),
        }
    }

    fn poly_position(id: &str, token: &str, market: &str, cost: f64) -> Position {
        let mut p = Position::new(
            id.into(),
            TradeSource::Polymarket,
            Venue::PolymarketClob,
            crate::models::ExecutionMode::Paper,
            token.into(),
            token.into(),
            "USDC".into(),
        );
        p.apply_buy(cost / 0.4, 0.4, cost);
        p.market_id = Some(market.into());
        p
    }

    #[tokio::test]
    async fn poly_emergency_disable_refuses_polymarket_entries_only() {
        let e = engine();
        e.state()
            .update_config(|c| c.risk.poly_emergency_disable = true)
            .await;
        let d = e.check_entry(&poly_request(5.0, 100.0)).await;
        assert_eq!(
            d.code,
            Some(RiskCode::PolyEmergencyDisabled),
            "{}",
            d.reason
        );
        let err = e.preflight_coded(BotModule::Polymarket).await.unwrap_err();
        assert_eq!(err.0.as_str(), "poly_emergency_disabled");
        // Sniper and copy untouched.
        assert!(e.check_entry(&request(0.01, 10.0)).await.allowed());
        let mut r = request(0.01, 10.0);
        r.module = BotModule::Copy;
        assert!(e.check_entry(&r).await.allowed());
        let risk = e.state().config_snapshot().await.risk;
        let coded = e.check_polymarket_coded(&risk, 5.0, 0, 0.0, 0.0).await;
        assert_eq!(coded.unwrap_err().0, RiskCode::PolyEmergencyDisabled);
    }

    #[tokio::test]
    async fn poly_daily_loss_limit_is_module_scoped() {
        let e = engine();
        // The generic (all-module, SOL-denominated) daily cap is switched
        // off here so only the Polymarket-scoped USDC cap is exercised.
        e.state()
            .update_config(|c| {
                c.risk.daily_loss_limit_quote = 0.0;
                c.risk.poly_daily_loss_limit_quote = 10.0;
            })
            .await;
        e.state().add_realized(BotModule::Copy, -50.0).await;
        assert!(e.check_entry(&poly_request(5.0, 100.0)).await.allowed());
        e.state().add_realized(BotModule::Polymarket, -10.0).await;
        let d = e.check_entry(&poly_request(5.0, 100.0)).await;
        assert_eq!(d.code, Some(RiskCode::PolyDailyLoss), "{}", d.reason);
        assert!(RiskCode::PolyDailyLoss.is_exposure_limit());
    }

    #[tokio::test]
    async fn poly_position_caps_and_order_book_controls_are_coded() {
        let e = engine();
        e.state()
            .update_config(|c| {
                c.risk.max_position_quote = 50.0;
                c.risk.max_open_positions = 8;
                c.risk.poly_max_position_quote = 20.0;
                c.risk.poly_max_concurrent_positions = 1;
                c.risk.poly_max_total_exposure_quote = 30.0;
                c.risk.poly_max_market_exposure_quote = 12.0;
                c.risk.poly_max_open_orders = 2;
            })
            .await;
        // Per-order cap resizes to 20.
        let d = e.check_entry(&poly_request(40.0, 1000.0)).await;
        assert!(d.allowed(), "{}", d.reason);
        assert!((d.sized_quote - 20.0).abs() < 1e-9, "{}", d.sized_quote);

        let risk = e.state().config_snapshot().await.risk;
        // Resting-order cap.
        let err = e
            .check_polymarket_coded(&risk, 5.0, 2, 4.0, 0.0)
            .await
            .unwrap_err();
        assert_eq!(err.0, RiskCode::PolyOpenOrderCap);
        assert!(err.0.is_exposure_limit());
        // Total envelope counts resting orders: 0 positions + 27 resting + 5 > 30.
        let err = e
            .check_polymarket_coded(&risk, 5.0, 1, 27.0, 0.0)
            .await
            .unwrap_err();
        assert_eq!(err.0, RiskCode::ExposureCap);
        // Per-market cap.
        let err = e
            .check_polymarket_coded(&risk, 5.0, 0, 0.0, 8.0)
            .await
            .unwrap_err();
        assert_eq!(err.0, RiskCode::PolyMarketExposure);
        assert_eq!(err.0.as_str(), "poly_market_exposure");
        // Within every cap.
        e.check_polymarket_coded(&risk, 5.0, 1, 4.0, 6.0)
            .await
            .unwrap();
        // Non-positive notional is invalid.
        assert_eq!(
            e.check_polymarket_coded(&risk, 0.0, 0, 0.0, 0.0)
                .await
                .unwrap_err()
                .0,
            RiskCode::InvalidSize
        );

        // Concurrent-position cap (1) is module scoped: an open polymarket
        // position blocks the next polymarket entry, not a sniper entry.
        e.state()
            .upsert_position(poly_position("pp1", "111", "0xm1", 5.0))
            .await;
        let d = e.check_entry(&poly_request(5.0, 1000.0)).await;
        assert_eq!(d.code, Some(RiskCode::MaxOpenPositions), "{}", d.reason);
        assert!(e.check_entry(&request(0.01, 10.0)).await.allowed());
        // The total envelope also sees the open position via open_exposure.
        let err = e
            .check_polymarket_coded(&risk, 5.0, 0, 21.0, 0.0)
            .await
            .unwrap_err();
        assert_eq!(err.0, RiskCode::ExposureCap, "{}", err.1);
    }
}
