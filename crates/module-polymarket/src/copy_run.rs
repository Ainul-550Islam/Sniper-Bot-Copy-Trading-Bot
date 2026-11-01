//! The copy-follow poller: the production call site of
//! [`crate::copy::CopyEngine`].
//!
//! Before this module existed, `copy.rs` (the decision engine) and
//! `copy_feed.rs` (the leader-trade source) both compiled and were both
//! unit-tested, but **nothing ever connected them to the bot**: the engine was
//! only ever constructed inside `copy.rs`'s own tests. "Copy a wallet" was,
//! in practice, configuration with no consumer.
//!
//! [`PolyBot::copy_poll_once`] is that consumer. Once per configured interval
//! ([`bot_core::config::PolymarketConfig::copy_poll_interval_secs`]) it:
//!
//! 1. polls the public data API once per followed wallet
//!    ([`crate::copy_feed::CopyFeedClient::recent_trades`]);
//! 2. resolves each leader fill's market — the scanner's cache first, then the
//!    Gamma catalogue by condition id;
//! 3. prices the outcome through the SAME quote path the strategy uses
//!    ([`PolyBot::quotes_for`]: websocket cache first, CLOB REST fallback);
//! 4. asks the engine for a verdict ([`crate::copy::CopyEngine::evaluate`]);
//! 5. runs every `Follow` signal through the staged pipeline
//!    ([`PolyBot::process_signal`]) — i.e. copy entries pass the exact same
//!    risk/exposure/collateral/idempotency/ownership/signing gates as strategy
//!    entries. This module performs **no** risk or money decision of its own.
//!
//! Honesty rules honoured here:
//!
//! * A feed failure is a logged error, never "no new trades".
//! * Every non-follow is counted with its explicit
//!   [`CopySkipReason`](crate::copy::CopySkipReason) label, so a tenant can see
//!   why a leader trade was not mirrored.
//! * Nothing is marked seen without passing through the engine, so a market
//!   that could not be priced this poll is re-evaluated on the next one.
//! * `liquidity_usd` is not reported by the trades endpoint; when the market
//!   resolves, the **Gamma market snapshot's** liquidity is used (a real
//!   published figure for that condition id) — never a guess. When the market
//!   cannot be resolved the value stays `0.0` and the engine's own liquidity
//!   gate decides.

use std::collections::HashMap;

use chrono::Utc;
use tracing::{debug, info, warn};

use bot_core::config::PolymarketConfig;
use bot_core::models::PolyMarket;

use crate::copy::{CopyEngine, CopyFollowConfig, CopySizing, CopyVerdict};
use crate::copy_feed::{CopyFeedClient, DEFAULT_TRADE_LIMIT};
use crate::metrics;
use crate::strategy::Quote;
use crate::PolyBot;

/// Strategy label copy signals are journaled under
/// ([`crate::copy::COPY_STRATEGY`]).
const COPY_STRATEGY: &str = crate::copy::COPY_STRATEGY;

/// What one poll did. Counters only — nothing here is a money figure.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopyPollReport {
    /// Followed wallets polled.
    pub wallets: usize,
    /// Feeds that errored (HTTP/parse/contract change). Never treated as
    /// "no trades".
    pub feed_errors: usize,
    /// API rows returned across all wallets.
    pub rows: usize,
    /// Rows the parser had to drop (missing/invalid fields).
    pub skipped_rows: usize,
    /// Leader trades that reached the engine.
    pub evaluated: usize,
    /// Leader trades the engine refused, per
    /// [`CopySkipReason`](crate::copy::CopySkipReason) label.
    pub skips: Vec<(&'static str, usize)>,
    /// Signals handed to the pipeline.
    pub signals: usize,
    /// Of those, signals that reached (or may have reached) the venue.
    pub reached_venue: usize,
    /// Signals the pipeline rejected before the venue.
    pub rejected: usize,
    /// Leader trades whose market could not be resolved this poll (they were
    /// still evaluated — with no quote — and stay eligible next poll).
    pub unresolved_markets: usize,
}

impl CopyPollReport {
    fn bump_skip(&mut self, reason: &'static str) {
        if let Some(entry) = self.skips.iter_mut().find(|(r, _)| *r == reason) {
            entry.1 += 1;
        } else {
            self.skips.push((reason, 1));
        }
    }

    /// Number of leader trades that produced no order.
    pub fn skipped(&self) -> usize {
        self.skips.iter().map(|(_, n)| *n).sum()
    }
}

impl PolyBot {
    /// Build the copy engine from the operator's configuration.
    ///
    /// `None` when no wallet is followed (the default): no leader feed is
    /// polled and no copy order can ever exist. An invalid copy configuration
    /// is an error the caller logs and treats as "copy disabled" — it never
    /// degrades into a different sizing rule.
    pub fn copy_engine_from_config(
        poly: &PolymarketConfig,
    ) -> Result<Option<CopyEngine>, &'static str> {
        if poly.copy_followed_wallets.is_empty() {
            return Ok(None);
        }
        let config = CopyFollowConfig {
            followed_wallets: poly.copy_followed_wallets.clone(),
            sizing: CopySizing::MirrorLeader {
                multiplier: poly.copy_size_multiplier,
            },
            min_stake_usd: poly.copy_min_stake_usd,
            max_stake_usd: poly.copy_max_stake_usd,
            max_price: poly.copy_max_price,
            slippage_tolerance_bps: poly.copy_slippage_tolerance_bps,
            min_liquidity_usd: poly.min_liquidity_usd,
            max_spread: poly.max_spread,
            quote_max_age_secs: poly.quote_max_age_secs,
        };
        CopyEngine::new(config).map(Some)
    }

    /// Poll every followed wallet once and mirror whatever the engine
    /// approves. Never returns an error: per-wallet failures are counted in
    /// the report and logged, so one unreachable feed cannot stop the others.
    pub async fn copy_poll_once(
        &self,
        engine: &mut CopyEngine,
        feed: &CopyFeedClient,
        poly: &PolymarketConfig,
    ) -> CopyPollReport {
        let mut report = CopyPollReport::default();
        let wallets = engine.config().followed_wallets.clone();
        report.wallets = wallets.len();
        if wallets.is_empty() {
            return report;
        }
        let mode = self.state.execution_mode().await;
        let now = Utc::now();

        for wallet in wallets {
            let page = match feed.recent_trades(&wallet, DEFAULT_TRADE_LIMIT).await {
                Ok(page) => page,
                Err(e) => {
                    report.feed_errors += 1;
                    warn!(wallet = %wallet, error = %e, "copy feed poll failed");
                    continue;
                }
            };
            report.rows += page.trades.len() + page.skipped;
            report.skipped_rows += page.skipped;
            if page.skipped > 0 {
                warn!(
                    wallet = %wallet,
                    skipped = page.skipped,
                    parsed = page.trades.len(),
                    "copy feed returned rows that could not be parsed (skipped, not faked)"
                );
            }

            // One quote map per condition id per poll: several leader fills can
            // land in the same market and must not re-hit the book endpoint.
            let mut quote_cache: HashMap<String, HashMap<String, Quote>> = HashMap::new();

            for mut trade in page.trades {
                // Cheap pre-checks so a replayed page row never triggers a book
                // fetch. `evaluate` remains the authority (it dedups first).
                if engine.seen(&trade.trade_id) {
                    continue;
                }
                if !trade.is_buy {
                    // Sells are an explicit skip, journaled by the engine.
                    report.evaluated += 1;
                    if let CopyVerdict::Skip(reason) = engine.evaluate(&trade, None, now, mode) {
                        report.bump_skip(reason.as_str());
                        metrics::count_strategy_skip(COPY_STRATEGY, reason.as_str());
                    }
                    continue;
                }

                let market = self.copy_trade_market(&trade.condition_id).await;
                if market.is_none() {
                    report.unresolved_markets += 1;
                }
                let quotes = match &market {
                    Some(market) => {
                        if let Some(cached) = quote_cache.get(&market.condition_id) {
                            cached.clone()
                        } else {
                            let quotes = self.quotes_for(market, poly).await;
                            quote_cache.insert(market.condition_id.clone(), quotes.clone());
                            quotes
                        }
                    }
                    None => HashMap::new(),
                };

                // Liquidity: the trades endpoint does not carry it. Use the
                // publishing market's figure when we resolved one; otherwise
                // leave the feed's 0.0 in place and let the engine's gate speak.
                if let Some(market) = &market {
                    if trade.liquidity_usd == 0.0 && market.liquidity.is_finite() {
                        trade.liquidity_usd = market.liquidity.max(0.0);
                    }
                    if trade.question.trim().is_empty() {
                        trade.question = market.question.clone();
                    }
                }
                // Tick size: not reported by the feed either. Ask the venue,
                // exactly as `build_signal` does for strategy entries.
                if trade.tick_size.trim().is_empty() {
                    trade.tick_size = self
                        .clob
                        .tick_size(&trade.condition_id)
                        .await
                        .unwrap_or_else(|_| "0.01".to_string());
                }

                let quote = quotes.get(&trade.token_id);
                report.evaluated += 1;
                let verdict = engine.evaluate(&trade, quote, now, mode);
                let signal = match verdict {
                    CopyVerdict::Skip(reason) => {
                        report.bump_skip(reason.as_str());
                        metrics::count_strategy_skip(COPY_STRATEGY, reason.as_str());
                        debug!(
                            trade = %trade.trade_id,
                            wallet = %trade.wallet,
                            reason = reason.as_str(),
                            "polymarket copy skip"
                        );
                        continue;
                    }
                    CopyVerdict::Follow(signal) => signal,
                };

                // A mirrored entry without a resolved market cannot be priced
                // or gated by the pipeline (it needs the market's resolution
                // date / status); it is refused here, explicitly.
                let Some(market) = market else {
                    report.bump_skip("UNRESOLVED_MARKET");
                    metrics::count_strategy_skip(COPY_STRATEGY, "UNRESOLVED_MARKET");
                    warn!(
                        trade = %trade.trade_id,
                        condition = %trade.condition_id,
                        "copy: mirrored trade has no resolvable market — not submitted"
                    );
                    continue;
                };

                report.signals += 1;
                let outcome = self.process_signal(&signal, &market, &quotes, poly).await;
                if outcome.reached_venue() {
                    report.reached_venue += 1;
                    info!(
                        trade = %trade.trade_id,
                        wallet = %trade.wallet,
                        token = %trade.token_id,
                        stage = outcome.stage.as_str(),
                        venue_order = outcome.venue_order_id.as_deref().unwrap_or("-"),
                        "polymarket copy signal reached the venue"
                    );
                } else {
                    report.rejected += 1;
                    debug!(
                        trade = %trade.trade_id,
                        stage = outcome.stage.as_str(),
                        reason = outcome.reject_reason.map(|r| r.as_str()).unwrap_or("-"),
                        detail = %outcome.detail,
                        "polymarket copy signal did not reach the venue"
                    );
                }
            }
        }
        report
    }

    /// Resolve the [`PolyMarket`] for a leader trade's condition id: the
    /// scanner's snapshot first (it is the same object the websocket feed and
    /// the pipeline gates use), then the Gamma catalogue.
    async fn copy_trade_market(&self, condition_id: &str) -> Option<PolyMarket> {
        if condition_id.trim().is_empty() {
            return None;
        }
        if let Some(market) = self.markets.read().await.get(condition_id).cloned() {
            return Some(market);
        }
        match self.gamma.market_by_condition(condition_id).await {
            Ok(Some(market)) => {
                // Cache it: quoting, the pipeline gates and the user feed all
                // read this map.
                self.markets
                    .write()
                    .await
                    .insert(market.condition_id.clone(), market.clone());
                Some(market)
            }
            Ok(None) => {
                debug!(condition = %condition_id, "copy: condition id not in the Gamma catalogue");
                None
            }
            Err(e) => {
                warn!(condition = %condition_id, error = %e, "copy: Gamma lookup failed");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copy::CopySkipReason;

    fn poly_config() -> PolymarketConfig {
        PolymarketConfig {
            copy_followed_wallets: vec!["0xLeader".into()],
            copy_size_multiplier: 0.5,
            copy_min_stake_usd: 2.0,
            copy_max_stake_usd: 20.0,
            copy_max_price: 0.8,
            copy_slippage_tolerance_bps: 250,
            ..PolymarketConfig::default()
        }
    }

    #[test]
    fn no_followed_wallets_means_no_engine() {
        let cfg = PolymarketConfig::default();
        assert!(PolyBot::copy_engine_from_config(&cfg).unwrap().is_none());
    }

    #[test]
    fn the_engine_is_built_from_the_operator_config() {
        let cfg = poly_config();
        let engine = PolyBot::copy_engine_from_config(&cfg)
            .unwrap()
            .expect("engine");
        let c = engine.config();
        assert_eq!(c.followed_wallets, vec!["0xLeader".to_string()]);
        assert_eq!(c.sizing, CopySizing::MirrorLeader { multiplier: 0.5 });
        assert_eq!(c.min_stake_usd, 2.0);
        assert_eq!(c.max_stake_usd, 20.0);
        assert_eq!(c.max_price, 0.8);
        assert_eq!(c.slippage_tolerance_bps, 250);
        // Market gates come from the shared polymarket config, not a second set
        // of knobs that could drift.
        assert_eq!(c.min_liquidity_usd, cfg.min_liquidity_usd);
        assert_eq!(c.max_spread, cfg.max_spread);
        assert_eq!(c.quote_max_age_secs, cfg.quote_max_age_secs);
        assert!(engine.follows("0xleader"));
        assert!(!engine.follows("0xother"));
    }

    #[test]
    fn an_invalid_copy_config_is_rejected_not_silently_reinterpreted() {
        let mut cfg = poly_config();
        cfg.copy_min_stake_usd = 0.0; // degenerate floor
        assert!(PolyBot::copy_engine_from_config(&cfg).is_err());

        let mut cfg = poly_config();
        cfg.copy_max_stake_usd = 1.0; // below the floor
        cfg.copy_min_stake_usd = 5.0;
        assert!(PolyBot::copy_engine_from_config(&cfg).is_err());

        let mut cfg = poly_config();
        cfg.copy_max_price = 1.5; // outside (0, 1]
        assert!(PolyBot::copy_engine_from_config(&cfg).is_err());
    }

    #[test]
    fn the_report_counts_every_skip_reason_separately() {
        let mut report = CopyPollReport::default();
        report.bump_skip(CopySkipReason::LeaderSell.as_str());
        report.bump_skip(CopySkipReason::LeaderSell.as_str());
        report.bump_skip(CopySkipReason::NoQuote.as_str());
        assert_eq!(report.skipped(), 3);
        assert_eq!(
            report
                .skips
                .iter()
                .find(|(r, _)| *r == "LEADER_SELL")
                .map(|(_, n)| *n),
            Some(2)
        );
        assert_eq!(
            report
                .skips
                .iter()
                .find(|(r, _)| *r == "NO_QUOTE")
                .map(|(_, n)| *n),
            Some(1)
        );
    }

    #[test]
    fn the_strategy_label_is_the_documented_copy_label() {
        assert_eq!(COPY_STRATEGY, "copy");
    }
}
