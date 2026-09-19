//! Betting strategies for Module 3.
//!
//! A strategy turns a [`PolyMarket`] plus a per-token [`Quote`] snapshot into
//! zero or more [`OrderDecision`]s. Two are implemented:
//!
//! * **`value`** — complementary mispricing. In a two-outcome market the best
//!   asks should sum to ~1.00 (buying every outcome redeems exactly 1.00). When
//!   `ask(A) + ask(B) < 1 - min_edge`, buying the whole basket locks in the
//!   edge, so we emit a buy per outcome. This is a real, model-free signal.
//! * **`search`** — keyword watchlist. If the market text matches one of
//!   `watch_keywords`, buy the favoured (YES) outcome at its ask, sized by
//!   `stake_usd`. A simple momentum/interest play.
//!
//! Everything is inert unless the configured strategy produces an edge, which
//! keeps paper trading quiet until there is something to do.

use std::collections::HashMap;

use bot_core::config::PolymarketConfig;
use bot_core::models::PolyMarket;

/// A one-token price snapshot from the CLOB book / websocket.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quote {
    /// Best bid (highest buy) in [0, 1].
    pub best_bid: f64,
    /// Best ask (lowest sell) in [0, 1].
    pub best_ask: f64,
    /// Midpoint in [0, 1].
    pub midpoint: f64,
}

impl Quote {
    /// A quote from a bid/ask pair (midpoint derived).
    pub fn new(best_bid: f64, best_ask: f64) -> Self {
        Quote {
            best_bid,
            best_ask,
            midpoint: (best_bid + best_ask) / 2.0,
        }
    }
}

/// A decision to place one order.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderDecision {
    /// The CTF token id to trade.
    pub token_id: String,
    /// Human outcome name ("Yes"/"No"/…).
    pub outcome: String,
    /// Buy (true) or sell (false).
    pub is_buy: bool,
    /// Size in outcome tokens.
    pub size_tokens: f64,
    /// Limit price in [0, 1].
    pub limit_price: f64,
    /// USDC notional this decision intends to commit.
    pub stake_usd: f64,
    /// The market's condition id (for order metadata / position tracking).
    pub condition_id: String,
    /// Whether the market is neg-risk (selects the exchange contract).
    pub neg_risk: bool,
    /// Human-readable rationale.
    pub reason: String,
}

/// Evaluate a market against the configured strategy.
pub fn evaluate(
    market: &PolyMarket,
    quotes: &HashMap<String, Quote>,
    cfg: &PolymarketConfig,
) -> Vec<OrderDecision> {
    if !market.active || market.closed || !market.accepting_orders {
        return Vec::new();
    }
    match cfg.strategy.trim().to_ascii_lowercase().as_str() {
        "value" | "arb" | "arbitrage" => value_strategy(market, quotes, cfg),
        "search" | "keyword" | "keywords" => search_strategy(market, quotes, cfg),
        other => {
            tracing::debug!(
                strategy = other,
                "unknown polymarket strategy; no decisions"
            );
            Vec::new()
        }
    }
}

/// Complementary-mispricing (basket) strategy for two-outcome markets.
fn value_strategy(
    market: &PolyMarket,
    quotes: &HashMap<String, Quote>,
    cfg: &PolymarketConfig,
) -> Vec<OrderDecision> {
    // Only binary markets have a clean "sum of asks == 1" invariant.
    if market.outcomes.len() != 2 {
        return Vec::new();
    }
    let a = &market.outcomes[0];
    let b = &market.outcomes[1];
    let (Some(qa), Some(qb)) = (quotes.get(&a.token_id), quotes.get(&b.token_id)) else {
        return Vec::new();
    };
    // Need a real ask on both sides.
    if !(qa.best_ask > 0.0 && qb.best_ask > 0.0) {
        return Vec::new();
    }
    let sum_asks = qa.best_ask + qb.best_ask;
    let edge = 1.0 - sum_asks;
    if edge < cfg.min_edge {
        return Vec::new();
    }

    // Buying one of each outcome costs `sum_asks` and redeems 1.00. Size the
    // basket so the total USDC committed is `stake_usd`.
    let baskets = cfg.stake_usd / sum_asks;
    let mut out = Vec::new();
    for (o, q) in [(a, qa), (b, qb)] {
        let size_tokens = round_size(baskets);
        if size_tokens <= 0.0 {
            continue;
        }
        out.push(OrderDecision {
            token_id: o.token_id.clone(),
            outcome: o.outcome.clone(),
            is_buy: true,
            size_tokens,
            limit_price: q.best_ask,
            stake_usd: size_tokens * q.best_ask,
            condition_id: market.condition_id.clone(),
            neg_risk: market.neg_risk,
            reason: format!(
                "basket edge {:.4} (asks {:.3}+{:.3}={:.3} < {:.3})",
                edge,
                qa.best_ask,
                qb.best_ask,
                sum_asks,
                1.0 - cfg.min_edge
            ),
        });
    }
    out
}

/// Keyword-watchlist strategy: buy the favoured outcome of matching markets.
fn search_strategy(
    market: &PolyMarket,
    quotes: &HashMap<String, Quote>,
    cfg: &PolymarketConfig,
) -> Vec<OrderDecision> {
    if cfg.watch_keywords.is_empty() {
        return Vec::new();
    }
    let haystack = format!("{} {}", market.question, market.slug).to_ascii_lowercase();
    let hit = cfg
        .watch_keywords
        .iter()
        .find(|k| !k.trim().is_empty() && haystack.contains(&k.trim().to_ascii_lowercase()));
    let Some(keyword) = hit else {
        return Vec::new();
    };

    // Favour the first outcome (conventionally "Yes").
    let Some(outcome) = market.outcomes.first() else {
        return Vec::new();
    };
    let Some(q) = quotes.get(&outcome.token_id) else {
        return Vec::new();
    };
    // Only enter when the ask is a sane, tradeable probability.
    if !(q.best_ask > 0.0 && q.best_ask < 0.99) {
        return Vec::new();
    }
    let size_tokens = round_size(cfg.stake_usd / q.best_ask);
    if size_tokens <= 0.0 {
        return Vec::new();
    }
    vec![OrderDecision {
        token_id: outcome.token_id.clone(),
        outcome: outcome.outcome.clone(),
        is_buy: true,
        size_tokens,
        limit_price: q.best_ask,
        stake_usd: cfg.stake_usd,
        condition_id: market.condition_id.clone(),
        neg_risk: market.neg_risk,
        reason: format!("keyword '{keyword}' @ ask {:.3}", q.best_ask),
    }]
}

/// Round a token size to 2 decimals (the CLOB size precision).
fn round_size(x: f64) -> f64 {
    (x * 100.0).floor() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::models::{PolyMarket, PolyOutcome};

    fn binary_market() -> PolyMarket {
        PolyMarket {
            condition_id: "0xcond".into(),
            question: "Will it rain?".into(),
            slug: "will-it-rain".into(),
            neg_risk: false,
            active: true,
            closed: false,
            accepting_orders: true,
            end_date: None,
            volume: 1000.0,
            liquidity: 500.0,
            outcomes: vec![
                PolyOutcome {
                    outcome: "Yes".into(),
                    token_id: "tokY".into(),
                    price: 0.4,
                    winner: None,
                },
                PolyOutcome {
                    outcome: "No".into(),
                    token_id: "tokN".into(),
                    price: 0.5,
                    winner: None,
                },
            ],
        }
    }

    fn cfg(strategy: &str, min_edge: f64) -> PolymarketConfig {
        PolymarketConfig {
            strategy: strategy.into(),
            min_edge,
            stake_usd: 10.0,
            watch_keywords: vec!["rain".into()],
            ..Default::default()
        }
    }

    #[test]
    fn value_detects_basket_edge() {
        let m = binary_market();
        let mut quotes = HashMap::new();
        quotes.insert("tokY".into(), Quote::new(0.38, 0.40));
        quotes.insert("tokN".into(), Quote::new(0.48, 0.50));
        // asks sum to 0.90 -> edge 0.10 >= 0.03
        let decisions = evaluate(&m, &quotes, &cfg("value", 0.03));
        assert_eq!(decisions.len(), 2);
        assert!(decisions.iter().all(|d| d.is_buy));
        assert!(decisions[0].reason.contains("basket edge"));
    }

    #[test]
    fn value_no_trade_when_asks_sum_above_one() {
        let m = binary_market();
        let mut quotes = HashMap::new();
        quotes.insert("tokY".into(), Quote::new(0.48, 0.50));
        quotes.insert("tokN".into(), Quote::new(0.50, 0.52));
        // 0.50 + 0.52 = 1.02 -> edge -0.02 < 0.03
        let decisions = evaluate(&m, &quotes, &cfg("value", 0.03));
        assert!(decisions.is_empty());
    }

    #[test]
    fn value_respects_min_edge_threshold() {
        let m = binary_market();
        let mut quotes = HashMap::new();
        quotes.insert("tokY".into(), Quote::new(0.38, 0.40));
        quotes.insert("tokN".into(), Quote::new(0.48, 0.50));
        // edge 0.10 < required 0.15 -> no trade
        let decisions = evaluate(&m, &quotes, &cfg("value", 0.15));
        assert!(decisions.is_empty());
    }

    #[test]
    fn search_buys_keyword_match() {
        let m = binary_market();
        let mut quotes = HashMap::new();
        quotes.insert("tokY".into(), Quote::new(0.38, 0.40));
        quotes.insert("tokN".into(), Quote::new(0.58, 0.60));
        let decisions = evaluate(&m, &quotes, &cfg("search", 0.03));
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].token_id, "tokY");
        assert!(decisions[0].reason.contains("keyword"));
        // size = 10 / 0.40 = 25 tokens
        assert!((decisions[0].size_tokens - 25.0).abs() < 0.011);
    }

    #[test]
    fn search_ignores_non_matching_market() {
        let mut m = binary_market();
        m.question = "Will it snow?".into();
        m.slug = "will-it-snow".into();
        let mut quotes = HashMap::new();
        quotes.insert("tokY".into(), Quote::new(0.38, 0.40));
        let decisions = evaluate(&m, &quotes, &cfg("search", 0.03));
        assert!(decisions.is_empty());
    }

    #[test]
    fn closed_market_produces_nothing() {
        let mut m = binary_market();
        m.closed = true;
        let quotes = HashMap::new();
        assert!(evaluate(&m, &quotes, &cfg("value", 0.03)).is_empty());
    }

    #[test]
    fn unknown_strategy_is_inert() {
        let m = binary_market();
        let quotes = HashMap::new();
        assert!(evaluate(&m, &quotes, &cfg("moonphase", 0.03)).is_empty());
    }
}
