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
//!
//! ## Hardened verdicts (TASK 4)
//! [`evaluate_market`] is the deterministic signal → decision function: it
//! takes an explicit clock, applies the market gate (active / accepting /
//! resolution window / liquidity) and the quote gate (two-sided book, spread,
//! staleness, price range) BEFORE the strategy logic, and returns one
//! [`Verdict`] per considered outcome — either `Enter(decision)` or `Skip`
//! with a machine-readable [`SkipReason`]. No decision is ever produced
//! silently and no rejection is ever swallowed. [`evaluate`] keeps the
//! original API (decisions only) as a thin wrapper.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

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
    /// When the venue produced this snapshot (book `timestamp`), if known.
    /// `None` = age unknown; the staleness gate treats it as fresh because
    /// REST fallbacks are fetched at decision time.
    pub observed_at: Option<DateTime<Utc>>,
}

impl Quote {
    /// A quote from a bid/ask pair (midpoint derived, age unknown).
    pub fn new(best_bid: f64, best_ask: f64) -> Self {
        Quote {
            best_bid,
            best_ask,
            midpoint: (best_bid + best_ask) / 2.0,
            observed_at: None,
        }
    }

    /// A quote with a known venue timestamp.
    pub fn observed(best_bid: f64, best_ask: f64, at: DateTime<Utc>) -> Self {
        Quote {
            observed_at: Some(at),
            ..Quote::new(best_bid, best_ask)
        }
    }

    /// Both sides present and not crossed.
    pub fn is_two_sided(&self) -> bool {
        self.best_bid > 0.0 && self.best_ask > 0.0 && self.best_ask >= self.best_bid
    }

    /// `ask - bid` in probability units (`0` when the book is one-sided).
    pub fn spread(&self) -> f64 {
        if self.is_two_sided() {
            self.best_ask - self.best_bid
        } else {
            0.0
        }
    }

    /// Age of the snapshot in seconds at `now`, when the timestamp is known.
    pub fn age_secs(&self, now: DateTime<Utc>) -> Option<i64> {
        self.observed_at
            .map(|at| now.signed_duration_since(at).num_seconds().max(0))
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

/// Why a market / outcome was NOT turned into a decision. Every variant is
/// explicit and stable (`as_str`) so it can be journaled and metered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkipReason {
    /// Gamma reports the market inactive.
    MarketInactive,
    /// The market is closed / resolved.
    MarketClosed,
    /// The CLOB is not accepting orders for this market.
    NotAcceptingOrders,
    /// The market resolves within `min_time_to_resolution_secs`.
    ResolvingSoon,
    /// Gamma liquidity below `min_liquidity_usd`.
    LowLiquidity,
    /// The `value` strategy needs exactly two outcomes.
    NotBinary,
    /// No quote for an outcome token.
    NoQuote,
    /// The book has no ask (nothing to buy) or no bid.
    OneSidedBook,
    /// `ask < bid` — a crossed / corrupt snapshot.
    CrossedBook,
    /// `ask - bid` above `max_spread`.
    SpreadTooWide,
    /// The quote is older than `quote_max_age_secs`.
    StaleQuote,
    /// The ask is outside the tradeable probability range.
    PriceOutOfRange,
    /// Basket edge below `min_edge`.
    NoEdge,
    /// `search` strategy: no keyword matched the market text.
    NoKeywordMatch,
    /// `search` strategy: `watch_keywords` is empty.
    NoKeywordsConfigured,
    /// Rounded size below `min_order_size`.
    SizeTooSmall,
    /// `stake_usd` is not a positive finite number.
    InvalidStake,
    /// The configured strategy name is not implemented.
    UnknownStrategy,
}

impl SkipReason {
    /// Stable machine-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            SkipReason::MarketInactive => "MARKET_INACTIVE",
            SkipReason::MarketClosed => "MARKET_CLOSED",
            SkipReason::NotAcceptingOrders => "NOT_ACCEPTING_ORDERS",
            SkipReason::ResolvingSoon => "RESOLVING_SOON",
            SkipReason::LowLiquidity => "LOW_LIQUIDITY",
            SkipReason::NotBinary => "NOT_BINARY",
            SkipReason::NoQuote => "NO_QUOTE",
            SkipReason::OneSidedBook => "ONE_SIDED_BOOK",
            SkipReason::CrossedBook => "CROSSED_BOOK",
            SkipReason::SpreadTooWide => "SPREAD_TOO_WIDE",
            SkipReason::StaleQuote => "STALE_QUOTE",
            SkipReason::PriceOutOfRange => "PRICE_OUT_OF_RANGE",
            SkipReason::NoEdge => "NO_EDGE",
            SkipReason::NoKeywordMatch => "NO_KEYWORD_MATCH",
            SkipReason::NoKeywordsConfigured => "NO_KEYWORDS_CONFIGURED",
            SkipReason::SizeTooSmall => "SIZE_TOO_SMALL",
            SkipReason::InvalidStake => "INVALID_STAKE",
            SkipReason::UnknownStrategy => "UNKNOWN_STRATEGY",
        }
    }

    /// Every variant, for exhaustive metering / docs.
    pub const ALL: [SkipReason; 18] = [
        SkipReason::MarketInactive,
        SkipReason::MarketClosed,
        SkipReason::NotAcceptingOrders,
        SkipReason::ResolvingSoon,
        SkipReason::LowLiquidity,
        SkipReason::NotBinary,
        SkipReason::NoQuote,
        SkipReason::OneSidedBook,
        SkipReason::CrossedBook,
        SkipReason::SpreadTooWide,
        SkipReason::StaleQuote,
        SkipReason::PriceOutOfRange,
        SkipReason::NoEdge,
        SkipReason::NoKeywordMatch,
        SkipReason::NoKeywordsConfigured,
        SkipReason::SizeTooSmall,
        SkipReason::InvalidStake,
        SkipReason::UnknownStrategy,
    ];
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The explicit outcome of evaluating one market / outcome.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Place this order.
    Enter(OrderDecision),
    /// Do not trade, and exactly why.
    Skip {
        /// Outcome token the verdict is about (`None` = whole market).
        token_id: Option<String>,
        /// Machine-readable reason.
        reason: SkipReason,
        /// Human-readable detail (numbers involved).
        detail: String,
    },
}

impl Verdict {
    fn skip(token_id: Option<&str>, reason: SkipReason, detail: impl Into<String>) -> Self {
        Verdict::Skip {
            token_id: token_id.map(str::to_string),
            reason,
            detail: detail.into(),
        }
    }

    /// The decision, when this verdict enters.
    pub fn decision(&self) -> Option<&OrderDecision> {
        match self {
            Verdict::Enter(d) => Some(d),
            Verdict::Skip { .. } => None,
        }
    }

    /// The skip reason, when this verdict skips.
    pub fn skip_reason(&self) -> Option<SkipReason> {
        match self {
            Verdict::Enter(_) => None,
            Verdict::Skip { reason, .. } => Some(*reason),
        }
    }
}

/// Canonical strategy label for `cfg.strategy` (`None` = unknown).
pub fn strategy_name(cfg: &PolymarketConfig) -> Option<&'static str> {
    match cfg.strategy.trim().to_ascii_lowercase().as_str() {
        "value" | "arb" | "arbitrage" => Some("value"),
        "search" | "keyword" | "keywords" => Some("search"),
        _ => None,
    }
}

/// Market-level gate: is this market tradeable at all right now?
pub fn market_gate(
    market: &PolyMarket,
    cfg: &PolymarketConfig,
    now: DateTime<Utc>,
) -> Result<(), (SkipReason, String)> {
    if !market.active {
        return Err((SkipReason::MarketInactive, "gamma active=false".into()));
    }
    if market.closed {
        return Err((SkipReason::MarketClosed, "gamma closed=true".into()));
    }
    if !market.accepting_orders {
        return Err((
            SkipReason::NotAcceptingOrders,
            "gamma acceptingOrders=false".into(),
        ));
    }
    if cfg.min_time_to_resolution_secs > 0 {
        if let Some(end) = market.end_date {
            let remaining = end.signed_duration_since(now).num_seconds();
            if remaining < cfg.min_time_to_resolution_secs {
                return Err((
                    SkipReason::ResolvingSoon,
                    format!(
                        "resolves in {remaining}s (< {}s)",
                        cfg.min_time_to_resolution_secs
                    ),
                ));
            }
        }
    }
    if cfg.min_liquidity_usd > 0.0 && market.liquidity < cfg.min_liquidity_usd {
        return Err((
            SkipReason::LowLiquidity,
            format!(
                "liquidity {:.2} < {:.2}",
                market.liquidity, cfg.min_liquidity_usd
            ),
        ));
    }
    Ok(())
}

/// `true` only for a finite, strictly positive price (NaN/inf/0 → false).
fn is_positive_price(x: f64) -> bool {
    x.is_finite() && x > 0.0
}

/// Quote-level gate for BUYING at the ask: a real two-sided, uncrossed,
/// tight enough, fresh enough book whose ask is a tradeable probability.
pub fn quote_gate(
    quote: &Quote,
    cfg: &PolymarketConfig,
    now: DateTime<Utc>,
) -> Result<(), (SkipReason, String)> {
    // Written with explicit finiteness checks so a NaN side (unparseable
    // book level) is rejected here instead of slipping through `>`.
    if !is_positive_price(quote.best_ask) || !is_positive_price(quote.best_bid) {
        return Err((
            SkipReason::OneSidedBook,
            format!("bid {:.4} ask {:.4}", quote.best_bid, quote.best_ask),
        ));
    }
    if quote.best_ask < quote.best_bid {
        return Err((
            SkipReason::CrossedBook,
            format!("ask {:.4} < bid {:.4}", quote.best_ask, quote.best_bid),
        ));
    }
    if !(0.01..0.99).contains(&quote.best_ask) {
        return Err((
            SkipReason::PriceOutOfRange,
            format!("ask {:.4} outside [0.01, 0.99)", quote.best_ask),
        ));
    }
    if cfg.max_spread > 0.0 && quote.spread() > cfg.max_spread + 1e-12 {
        return Err((
            SkipReason::SpreadTooWide,
            format!("spread {:.4} > {:.4}", quote.spread(), cfg.max_spread),
        ));
    }
    if cfg.quote_max_age_secs > 0 {
        if let Some(age) = quote.age_secs(now) {
            if age > cfg.quote_max_age_secs {
                return Err((
                    SkipReason::StaleQuote,
                    format!("quote {age}s old (> {}s)", cfg.quote_max_age_secs),
                ));
            }
        }
    }
    Ok(())
}

/// Evaluate a market against the configured strategy — decisions only.
///
/// Thin wrapper over [`evaluate_market`] at the current wall clock, kept for
/// callers that only want orders. Skips are still computed (and available
/// through [`evaluate_market`]) — nothing is decided implicitly.
pub fn evaluate(
    market: &PolyMarket,
    quotes: &HashMap<String, Quote>,
    cfg: &PolymarketConfig,
) -> Vec<OrderDecision> {
    evaluate_market(market, quotes, cfg, Utc::now())
        .into_iter()
        .filter_map(|v| match v {
            Verdict::Enter(d) => Some(d),
            Verdict::Skip { .. } => None,
        })
        .collect()
}

/// Deterministic signal → decision evaluation with explicit verdicts.
///
/// Same inputs (market, quotes, config, `now`) always produce the same
/// verdicts in the same order. Gates run first (market, then per-quote), the
/// strategy logic runs only on what passed.
pub fn evaluate_market(
    market: &PolyMarket,
    quotes: &HashMap<String, Quote>,
    cfg: &PolymarketConfig,
    now: DateTime<Utc>,
) -> Vec<Verdict> {
    if let Err((reason, detail)) = market_gate(market, cfg, now) {
        return vec![Verdict::skip(None, reason, detail)];
    }
    if !cfg.stake_usd.is_finite() || cfg.stake_usd <= 0.0 {
        return vec![Verdict::skip(
            None,
            SkipReason::InvalidStake,
            format!("stake_usd {}", cfg.stake_usd),
        )];
    }
    match strategy_name(cfg) {
        Some("value") => value_strategy(market, quotes, cfg, now),
        Some("search") => search_strategy(market, quotes, cfg, now),
        _ => {
            tracing::debug!(
                strategy = %cfg.strategy,
                "unknown polymarket strategy; no decisions"
            );
            vec![Verdict::skip(
                None,
                SkipReason::UnknownStrategy,
                format!("strategy '{}'", cfg.strategy.trim()),
            )]
        }
    }
}

/// Complementary-mispricing (basket) strategy for two-outcome markets.
fn value_strategy(
    market: &PolyMarket,
    quotes: &HashMap<String, Quote>,
    cfg: &PolymarketConfig,
    now: DateTime<Utc>,
) -> Vec<Verdict> {
    // Only binary markets have a clean "sum of asks == 1" invariant.
    if market.outcomes.len() != 2 {
        return vec![Verdict::skip(
            None,
            SkipReason::NotBinary,
            format!("{} outcomes", market.outcomes.len()),
        )];
    }
    let a = &market.outcomes[0];
    let b = &market.outcomes[1];
    let mut out = Vec::new();
    let mut gated = false;
    for o in [a, b] {
        match quotes.get(&o.token_id) {
            None => {
                out.push(Verdict::skip(
                    Some(&o.token_id),
                    SkipReason::NoQuote,
                    "no book snapshot",
                ));
                gated = true;
            }
            Some(q) => {
                if let Err((reason, detail)) = quote_gate(q, cfg, now) {
                    out.push(Verdict::skip(Some(&o.token_id), reason, detail));
                    gated = true;
                }
            }
        }
    }
    if gated {
        return out;
    }
    let (qa, qb) = (quotes[&a.token_id], quotes[&b.token_id]);
    let sum_asks = qa.best_ask + qb.best_ask;
    let edge = 1.0 - sum_asks;
    if edge < cfg.min_edge {
        return vec![Verdict::skip(
            None,
            SkipReason::NoEdge,
            format!(
                "basket edge {:.4} (asks {:.3}+{:.3}={:.3}) < min_edge {:.4}",
                edge, qa.best_ask, qb.best_ask, sum_asks, cfg.min_edge
            ),
        )];
    }

    // Buying one of each outcome costs `sum_asks` and redeems 1.00. Size the
    // basket so the total USDC committed is `stake_usd`.
    let baskets = cfg.stake_usd / sum_asks;
    for (o, q) in [(a, qa), (b, qb)] {
        let size_tokens = round_size(baskets);
        if size_tokens <= 0.0 || size_tokens < cfg.min_order_size {
            out.push(Verdict::skip(
                Some(&o.token_id),
                SkipReason::SizeTooSmall,
                format!(
                    "size {size_tokens:.2} < min_order_size {:.2}",
                    cfg.min_order_size
                ),
            ));
            continue;
        }
        out.push(Verdict::Enter(OrderDecision {
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
        }));
    }
    out
}

/// Keyword-watchlist strategy: buy the favoured outcome of matching markets.
fn search_strategy(
    market: &PolyMarket,
    quotes: &HashMap<String, Quote>,
    cfg: &PolymarketConfig,
    now: DateTime<Utc>,
) -> Vec<Verdict> {
    if cfg.watch_keywords.iter().all(|k| k.trim().is_empty()) {
        return vec![Verdict::skip(
            None,
            SkipReason::NoKeywordsConfigured,
            "watch_keywords empty",
        )];
    }
    let haystack = format!("{} {}", market.question, market.slug).to_ascii_lowercase();
    let hit = cfg
        .watch_keywords
        .iter()
        .find(|k| !k.trim().is_empty() && haystack.contains(&k.trim().to_ascii_lowercase()));
    let Some(keyword) = hit else {
        return vec![Verdict::skip(
            None,
            SkipReason::NoKeywordMatch,
            "no watch keyword in question/slug",
        )];
    };

    // Favour the first outcome (conventionally "Yes").
    let Some(outcome) = market.outcomes.first() else {
        return vec![Verdict::skip(
            None,
            SkipReason::NoQuote,
            "market has no outcomes",
        )];
    };
    let Some(q) = quotes.get(&outcome.token_id) else {
        return vec![Verdict::skip(
            Some(&outcome.token_id),
            SkipReason::NoQuote,
            "no book snapshot",
        )];
    };
    // Only enter when the book is real and the ask is a sane, tradeable
    // probability.
    if let Err((reason, detail)) = quote_gate(q, cfg, now) {
        return vec![Verdict::skip(Some(&outcome.token_id), reason, detail)];
    }
    let size_tokens = round_size(cfg.stake_usd / q.best_ask);
    if size_tokens <= 0.0 || size_tokens < cfg.min_order_size {
        return vec![Verdict::skip(
            Some(&outcome.token_id),
            SkipReason::SizeTooSmall,
            format!(
                "size {size_tokens:.2} < min_order_size {:.2}",
                cfg.min_order_size
            ),
        )];
    }
    vec![Verdict::Enter(OrderDecision {
        token_id: outcome.token_id.clone(),
        outcome: outcome.outcome.clone(),
        is_buy: true,
        size_tokens,
        limit_price: q.best_ask,
        stake_usd: cfg.stake_usd,
        condition_id: market.condition_id.clone(),
        neg_risk: market.neg_risk,
        reason: format!("keyword '{keyword}' @ ask {:.3}", q.best_ask),
    })]
}

/// Round a token size to 2 decimals (the CLOB size precision).
pub fn round_size(x: f64) -> f64 {
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

    // ------------------------------------------------------------------
    // Hardened verdicts (TASK 4)
    // ------------------------------------------------------------------

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-21T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn reasons(verdicts: &[Verdict]) -> Vec<SkipReason> {
        verdicts.iter().filter_map(Verdict::skip_reason).collect()
    }

    #[test]
    fn verdicts_are_explicit_for_every_market_gate() {
        let c = cfg("value", 0.03);
        let quotes = HashMap::new();
        let mut m = binary_market();
        m.active = false;
        assert_eq!(
            reasons(&evaluate_market(&m, &quotes, &c, now())),
            vec![SkipReason::MarketInactive]
        );
        let mut m = binary_market();
        m.closed = true;
        assert_eq!(
            reasons(&evaluate_market(&m, &quotes, &c, now())),
            vec![SkipReason::MarketClosed]
        );
        let mut m = binary_market();
        m.accepting_orders = false;
        assert_eq!(
            reasons(&evaluate_market(&m, &quotes, &c, now())),
            vec![SkipReason::NotAcceptingOrders]
        );
        // Resolution window: default 3600 s.
        let mut m = binary_market();
        m.end_date = Some(now() + chrono::Duration::minutes(30));
        let v = evaluate_market(&m, &quotes, &c, now());
        assert_eq!(reasons(&v), vec![SkipReason::ResolvingSoon]);
        assert!(matches!(&v[0], Verdict::Skip { detail, .. } if detail.contains("1800s")));
        // …and off when 0.
        let mut c0 = c.clone();
        c0.min_time_to_resolution_secs = 0;
        assert_eq!(
            reasons(&evaluate_market(&m, &quotes, &c0, now())),
            vec![SkipReason::NoQuote, SkipReason::NoQuote]
        );
        // Liquidity floor.
        let mut cl = c.clone();
        cl.min_liquidity_usd = 1_000.0;
        assert_eq!(
            reasons(&evaluate_market(&binary_market(), &quotes, &cl, now())),
            vec![SkipReason::LowLiquidity]
        );
        // Unknown strategy / invalid stake.
        assert_eq!(
            reasons(&evaluate_market(
                &binary_market(),
                &quotes,
                &cfg("moonphase", 0.03),
                now()
            )),
            vec![SkipReason::UnknownStrategy]
        );
        let mut cs = c.clone();
        cs.stake_usd = f64::NAN;
        assert_eq!(
            reasons(&evaluate_market(&binary_market(), &quotes, &cs, now())),
            vec![SkipReason::InvalidStake]
        );
    }

    #[test]
    fn quote_gate_rejects_one_sided_crossed_wide_stale_and_out_of_range() {
        let c = cfg("value", 0.03);
        let t = now();
        let err = |q: Quote| quote_gate(&q, &c, t).unwrap_err().0;
        assert_eq!(err(Quote::new(0.0, 0.40)), SkipReason::OneSidedBook);
        assert_eq!(err(Quote::new(0.40, 0.0)), SkipReason::OneSidedBook);
        assert_eq!(err(Quote::new(0.45, 0.40)), SkipReason::CrossedBook);
        assert_eq!(err(Quote::new(0.98, 0.995)), SkipReason::PriceOutOfRange);
        assert_eq!(err(Quote::new(0.30, 0.45)), SkipReason::SpreadTooWide);
        assert_eq!(
            err(Quote::observed(
                0.38,
                0.40,
                t - chrono::Duration::seconds(121)
            )),
            SkipReason::StaleQuote
        );
        // Fresh, unknown-age and boundary spread pass.
        assert!(quote_gate(&Quote::new(0.38, 0.40), &c, t).is_ok());
        assert!(quote_gate(
            &Quote::observed(0.38, 0.40, t - chrono::Duration::seconds(120)),
            &c,
            t
        )
        .is_ok());
        assert!(quote_gate(&Quote::new(0.30, 0.40), &c, t).is_ok());
        // Spread gate off.
        let mut c0 = c.clone();
        c0.max_spread = 0.0;
        assert!(quote_gate(&Quote::new(0.10, 0.60), &c0, t).is_ok());
        // Staleness gate off.
        let mut c1 = c.clone();
        c1.quote_max_age_secs = 0;
        assert!(quote_gate(
            &Quote::observed(0.38, 0.40, t - chrono::Duration::hours(5)),
            &c1,
            t
        )
        .is_ok());
        let q = Quote::observed(0.38, 0.40, t - chrono::Duration::seconds(7));
        assert_eq!(q.age_secs(t), Some(7));
        assert!((q.spread() - 0.02).abs() < 1e-12);
        assert_eq!(Quote::new(0.0, 0.5).spread(), 0.0);
    }

    #[test]
    fn value_verdicts_report_every_outcome_and_are_deterministic() {
        let m = binary_market();
        let c = cfg("value", 0.03);
        let mut quotes = HashMap::new();
        quotes.insert("tokY".into(), Quote::new(0.38, 0.40));
        quotes.insert("tokN".into(), Quote::new(0.48, 0.50));
        let a = evaluate_market(&m, &quotes, &c, now());
        let b = evaluate_market(&m, &quotes, &c, now());
        assert_eq!(a, b, "same inputs → same verdicts");
        assert_eq!(a.len(), 2);
        assert!(a.iter().all(|v| v.decision().is_some()));
        assert_eq!(a[0].decision().unwrap().token_id, "tokY");
        assert_eq!(a[1].decision().unwrap().token_id, "tokN");

        // One stale leg gates the whole basket (never buy half a basket).
        quotes.insert(
            "tokN".into(),
            Quote::observed(0.48, 0.50, now() - chrono::Duration::hours(1)),
        );
        let v = evaluate_market(&m, &quotes, &c, now());
        assert_eq!(reasons(&v), vec![SkipReason::StaleQuote]);
        assert!(matches!(&v[0], Verdict::Skip { token_id: Some(t), .. } if t == "tokN"));

        // No edge is explicit.
        quotes.insert("tokN".into(), Quote::new(0.58, 0.60));
        assert_eq!(
            reasons(&evaluate_market(&m, &quotes, &c, now())),
            vec![SkipReason::NoEdge]
        );

        // Non-binary market.
        let mut m3 = binary_market();
        m3.outcomes.push(PolyOutcome {
            outcome: "Maybe".into(),
            token_id: "tokM".into(),
            price: 0.1,
            winner: None,
        });
        assert_eq!(
            reasons(&evaluate_market(&m3, &quotes, &c, now())),
            vec![SkipReason::NotBinary]
        );

        // Minimum order size.
        let mut small = c.clone();
        small.stake_usd = 2.0; // 2 / 0.9 = 2.22 tokens < 5
        quotes.insert("tokN".into(), Quote::new(0.48, 0.50));
        assert_eq!(
            reasons(&evaluate_market(&m, &quotes, &small, now())),
            vec![SkipReason::SizeTooSmall, SkipReason::SizeTooSmall]
        );
    }

    #[test]
    fn search_verdicts_are_explicit() {
        let m = binary_market();
        let mut quotes = HashMap::new();
        quotes.insert("tokY".into(), Quote::new(0.38, 0.40));
        let mut c = cfg("search", 0.03);
        c.watch_keywords = vec!["  ".into()];
        assert_eq!(
            reasons(&evaluate_market(&m, &quotes, &c, now())),
            vec![SkipReason::NoKeywordsConfigured]
        );
        c.watch_keywords = vec!["snow".into()];
        assert_eq!(
            reasons(&evaluate_market(&m, &quotes, &c, now())),
            vec![SkipReason::NoKeywordMatch]
        );
        c.watch_keywords = vec!["RAIN".into()];
        let v = evaluate_market(&m, &quotes, &c, now());
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].decision().unwrap().token_id, "tokY");
        quotes.insert("tokY".into(), Quote::new(0.20, 0.40));
        assert_eq!(
            reasons(&evaluate_market(&m, &quotes, &c, now())),
            vec![SkipReason::SpreadTooWide]
        );
        quotes.clear();
        assert_eq!(
            reasons(&evaluate_market(&m, &quotes, &c, now())),
            vec![SkipReason::NoQuote]
        );
    }

    #[test]
    fn skip_reasons_have_unique_stable_labels() {
        let mut seen = std::collections::HashSet::new();
        for r in SkipReason::ALL {
            assert!(seen.insert(r.as_str()), "duplicate label {r}");
            assert!(r
                .as_str()
                .chars()
                .all(|c| c.is_ascii_uppercase() || c == '_'));
        }
        assert_eq!(seen.len(), 18);
        assert_eq!(strategy_name(&cfg("ARB", 0.0)), Some("value"));
        assert_eq!(strategy_name(&cfg("keywords", 0.0)), Some("search"));
        assert_eq!(strategy_name(&cfg("mm", 0.0)), None);
    }
}
