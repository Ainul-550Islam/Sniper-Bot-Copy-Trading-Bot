//! Market discovery and pricing — the front of the engine.
//!
//! One scan ([`PolyBot::scan_once`]) asks Gamma for the active catalogue,
//! prices every outcome token (market-websocket cache first, CLOB REST book
//! as the fallback), hands each market to the strategy for explicit verdicts
//! ([`PolyBot::process_market`]) and freezes every `Enter` verdict into an
//! [`OrderSignal`] ([`PolyBot::build_signal`]) before the staged pipeline
//! runs it. Skips are metered (`poly_strategy_skips_total{strategy,reason}`);
//! decisions are journaled by the pipeline.

use std::collections::HashMap;

use chrono::Utc;
use tracing::debug;

use bot_core::config::PolymarketConfig;
use bot_core::models::PolyMarket;

use crate::error::PolyResult;
use crate::gamma::MarketQuery;
use crate::metrics;
use crate::orders::{OrderSignal, SignalOutcome};
use crate::strategy::{evaluate_market, strategy_name, OrderDecision, Quote, Verdict};
use crate::PolyBot;

impl PolyBot {
    /// One catalogue scan: discover → price → decide → act. Returns the set of
    /// token ids we are now tracking (for the websocket).
    pub(crate) async fn scan_once(&self, poly: &PolymarketConfig) -> PolyResult<Vec<String>> {
        let markets = self.discover(poly).await?;
        if markets.is_empty() {
            return Ok(Vec::new());
        }

        let mut tracked = Vec::new();
        let mut acted = 0usize;

        for market in &markets {
            for o in &market.outcomes {
                tracked.push(o.token_id.clone());
            }
            self.markets
                .write()
                .await
                .insert(market.condition_id.clone(), market.clone());
            let quotes = self.quotes_for(market, poly).await;
            if quotes.is_empty() {
                continue;
            }
            if acted >= poly.max_open_markets.saturating_mul(2) {
                continue;
            }
            for outcome in self.process_market(market, &quotes, poly).await {
                if outcome.reached_venue() {
                    acted += 1;
                }
            }
        }
        Ok(tracked)
    }

    /// Evaluate one market (explicit verdicts) and run every entering
    /// decision through the pipeline. Skips are metered; decisions are
    /// journaled by [`PolyBot::process_signal`].
    pub async fn process_market(
        &self,
        market: &PolyMarket,
        quotes: &HashMap<String, Quote>,
        poly: &PolymarketConfig,
    ) -> Vec<SignalOutcome> {
        let now = Utc::now();
        let strategy = strategy_name(poly).unwrap_or("unknown");
        let mut outcomes = Vec::new();
        for verdict in evaluate_market(market, quotes, poly, now) {
            match verdict {
                Verdict::Skip {
                    token_id,
                    reason,
                    detail,
                } => {
                    metrics::count_strategy_skip(strategy, reason.as_str());
                    debug!(
                        market = %market.condition_id,
                        token = token_id.as_deref().unwrap_or("-"),
                        reason = %reason,
                        detail = %detail,
                        "polymarket strategy skip"
                    );
                }
                Verdict::Enter(decision) => {
                    let signal = self.build_signal(decision, strategy, market, poly).await;
                    let outcome = self.process_signal(&signal, market, quotes, poly).await;
                    outcomes.push(outcome);
                }
            }
        }
        outcomes
    }

    /// Freeze a decision into an [`OrderSignal`] (tick size from the venue,
    /// GTD expiry from config, current execution mode).
    pub async fn build_signal(
        &self,
        decision: OrderDecision,
        strategy: &'static str,
        market: &PolyMarket,
        poly: &PolymarketConfig,
    ) -> OrderSignal {
        let mode = self.state.execution_mode().await;
        let tick = self
            .clob
            .tick_size(&market.condition_id)
            .await
            .unwrap_or_else(|_| "0.01".into());
        let expiration = if poly.order_type.eq_ignore_ascii_case("GTD") && poly.expiration_secs > 0
        {
            (Utc::now().timestamp() + poly.expiration_secs).max(0) as u64
        } else {
            0
        };
        OrderSignal::new(
            decision,
            strategy,
            mode,
            &poly.order_type,
            &tick,
            expiration,
            &market.question,
            Utc::now(),
        )
    }

    /// Discover candidate markets per the configured strategy.
    async fn discover(&self, _poly: &PolymarketConfig) -> PolyResult<Vec<PolyMarket>> {
        let query = MarketQuery {
            active: Some(true),
            closed: Some(false),
            limit: Some(50),
            order: Some("volume24hr".into()),
            ascending: Some(false),
            ..Default::default()
        };
        // The search strategy narrows by keyword client-side; keep the query
        // broad and let `evaluate` filter.
        self.gamma.markets(&query).await
    }

    /// Build a per-token quote map for a market: websocket first, REST fallback.
    async fn quotes_for(
        &self,
        market: &PolyMarket,
        _poly: &PolymarketConfig,
    ) -> HashMap<String, Quote> {
        let mut out = HashMap::new();
        let cached = self.quotes.read().await;
        for o in &market.outcomes {
            if let Some(q) = cached.get(&o.token_id) {
                out.insert(o.token_id.clone(), *q);
            }
        }
        drop(cached);

        // Fill any gaps from the REST book.
        for o in &market.outcomes {
            if out.contains_key(&o.token_id) {
                continue;
            }
            match self.clob.order_book(&o.token_id).await {
                Ok(book) => {
                    out.insert(o.token_id.clone(), book.to_quote());
                }
                Err(e) => debug!(token = %o.token_id, error = %e, "could not fetch book"),
            }
        }
        out
    }
}
