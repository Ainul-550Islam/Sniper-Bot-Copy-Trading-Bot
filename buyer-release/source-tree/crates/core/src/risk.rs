//! The risk engine. Every execution path — sniper, copy trader, Polymarket —
//! must call [`RiskEngine::check_entry`] before building an order and
//! [`RiskEngine::check_exit`] on every price update.
//!
//! The engine is deliberately conservative: anything it cannot verify is
//! rejected, and the kill switch short-circuits every check.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde::Serialize;

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

#[derive(Debug, Clone, Serialize)]
pub struct RiskDecision {
    pub verdict: RiskVerdict,
    pub reason: String,
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

    fn reject<S: Into<String>>(requested: f64, reason: S) -> Self {
        RiskDecision {
            verdict: RiskVerdict::Reject,
            reason: reason.into(),
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
        if self.state.kill_switch() {
            return Err("kill switch engaged".into());
        }
        if !self.state.is_enabled(module).await {
            return Err(format!("{module} is disabled"));
        }
        let risk = self.risk_config().await;
        let daily = self.state.daily_stats().await;
        if daily.loss_limit_tripped {
            return Err(format!(
                "daily loss limit tripped at {}",
                daily
                    .loss_limit_tripped_at
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_else(|| "unknown".into())
            ));
        }
        if risk.daily_loss_limit_quote > 0.0 {
            let realized = self.effective_realized(daily.realized_pnl).await;
            if realized <= -risk.daily_loss_limit_quote {
                return Err(format!(
                    "daily realized loss {:.4} exceeds limit {:.4}",
                    realized, risk.daily_loss_limit_quote
                ));
            }
        }
        Ok(())
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

        // 1. Kill switch / module / daily loss -----------------------------
        if let Err(reason) = self.preflight(req.module).await {
            return RiskDecision::reject(req.requested_quote, reason);
        }

        // 2. Request sanity ------------------------------------------------
        // NaN must reject: matches!-on-partial_cmp makes that explicit.
        if !matches!(
            req.requested_quote.partial_cmp(&0.0),
            Some(std::cmp::Ordering::Greater)
        ) || !req.requested_quote.is_finite()
        {
            return RiskDecision::reject(req.requested_quote, "requested size must be > 0");
        }

        // 3. Slippage cap --------------------------------------------------
        if req.slippage_bps > risk.max_slippage_bps {
            return RiskDecision::reject(
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
        //    what caps global exposure (§Q).
        let open = self.state.open_positions_for(req.module).await;
        let open_count = self.effective_open_count(req.module, open.len()).await;
        if open_count >= risk.max_open_positions {
            return RiskDecision::reject(
                req.requested_quote,
                format!(
                    "already {} open positions (max {})",
                    open_count, risk.max_open_positions
                ),
            );
        }

        // 5. Duplicate symbol ---------------------------------------------
        if open.iter().any(|p| p.symbol == req.symbol) {
            return RiskDecision::reject(
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
                req.requested_quote,
                format!(
                    "re-entry cooldown active ({}s, last exit {at})",
                    risk.reentry_cooldown_secs
                ),
            );
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
        let mut sized = req
            .requested_quote
            .min(risk.max_position_quote)
            .min(fraction_cap);

        // Never let total exposure exceed what the daily loss limit can absorb
        // times a safety factor, so one bad candle cannot blow the account.
        let exposure_cap =
            (risk.max_position_quote * risk.max_open_positions as f64).max(risk.max_position_quote);
        if exposure + sized > exposure_cap {
            sized = (exposure_cap - exposure).max(0.0);
        }

        if sized > spendable {
            sized = spendable;
        }
        if sized <= 0.0 {
            return RiskDecision::reject(
                req.requested_quote,
                "computed size is zero after caps (exposure or balance limit reached)",
            );
        }

        let verdict = if (sized - req.requested_quote).abs() < 1e-12 {
            RiskVerdict::Allow
        } else {
            RiskVerdict::AllowReduced
        };
        let reason = match verdict {
            RiskVerdict::AllowReduced => format!(
                "size reduced {:.6} -> {:.6} (max_position_quote={:.4}, fraction_cap={:.4}, spendable={:.4})",
                req.requested_quote, sized, risk.max_position_quote, fraction_cap, spendable
            ),
            _ => "ok".into(),
        };

        // 9. Venue-specific checks -----------------------------------------
        if matches!(req.venue, Venue::PolymarketClob) {
            if let Some(price) = req.price {
                if !(price > 0.0 && price < 1.0) {
                    return RiskDecision::reject(
                        req.requested_quote,
                        format!("price {price} outside (0, 1)"),
                    );
                }
                if price < risk.poly_price_floor || price > risk.poly_price_ceiling {
                    return RiskDecision::reject(
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
            sized_quote: sized,
            requested_quote: req.requested_quote,
            stop_loss: stop_loss.filter(|v| v.is_finite()),
            take_profit: take_profit.filter(|v| v.is_finite()),
            trailing_stop_pct: risk.trailing_stop_pct,
            max_hold_secs: risk.max_hold_secs,
        }
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
        self.preflight(BotModule::Copy).await?;
        if trade.sol_amount <= 0.0 {
            return Err("whale trade has no SOL amount".into());
        }
        if !self.state.copy_allowed(&trade.wallet, &trade.mint).await {
            return Err(format!(
                "copy cooldown active for {} on {}",
                trade.wallet, trade.mint
            ));
        }
        let staleness = Utc::now()
            .signed_duration_since(trade.observed_at)
            .num_seconds();
        if staleness > risk.copy_cooldown_secs.max(30) {
            return Err(format!("whale trade is {staleness}s stale"));
        }
        Ok(trade.sol_amount)
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
}
