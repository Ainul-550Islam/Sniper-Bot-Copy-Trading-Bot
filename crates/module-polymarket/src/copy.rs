//! Copy-follow trading on Polymarket (GAP-MAP v2, P2).
//!
//! Follow SELECTED wallets: when a followed leader trades, decide whether
//! to mirror the trade under the tenant's sizing / slippage / market
//! filters, and emit a frozen [`OrderSignal`] that enters the SAME staged
//! pipeline ([`crate::pipeline`]) as the value/search strategies. This
//! module does not re-implement risk, exposure or OMS gates — the pipeline
//! applies all of them to copy signals exactly as it does to strategy
//! signals (one risk engine, one idempotency key, one journal).
//!
//! Design decisions (kept deliberately conservative):
//! * **buys only**: following a leader's SELL would require knowing OUR
//!   position in that outcome (the pipeline owns positions, not this
//!   module); v1 mirrors entries, exits stay with the lifecycle/strategy
//!   layer. A leader sell is an explicit skip, never an implicit close.
//! * **explicit skip vocabulary** ([`CopySkipReason`]) mirroring
//!   [`crate::strategy::SkipReason`]: every non-follow is journaled, so a
//!   tenant can see exactly why a leader trade was not mirrored.
//! * **dedup by venue trade id**: the same leader trade replayed by a
//!   reconnecting feed is never mirrored twice.
//! * **no float in ranking/caps**: USDC quantities are f64 only at the
//!   venue boundary (the module-wide convention); every comparison that
//!   gates money is against a config bound, never against accumulated
//!   state.

use std::collections::{HashSet, VecDeque};

use chrono::{DateTime, Utc};

use bot_core::models::ExecutionMode;

use crate::orders::OrderSignal;
use crate::strategy::{round_size, OrderDecision, Quote};

/// Strategy label copy signals carry through the pipeline/journal.
pub const COPY_STRATEGY: &str = "copy";

/// Capacity of the seen-trade dedup ring (per engine). A reconnecting feed
/// replaying history must never re-fire a mirrored trade; 20k ids covers
/// any realistic replay while bounding memory.
const SEEN_TRADE_CAP: usize = 20_000;

/// How a follow order is sized.
#[derive(Debug, Clone, PartialEq)]
pub enum CopySizing {
    /// Fixed USDC stake per mirrored trade.
    FixedUsd(f64),
    /// Mirror the leader's stake times a multiplier (0.5 = half size).
    /// The result is still clamped by the config's min/max stake.
    MirrorLeader { multiplier: f64 },
}

/// Tenant-facing copy configuration (server maps this from the SaaS
/// strategy config; every bound fails closed when absent or degenerate).
#[derive(Debug, Clone, PartialEq)]
pub struct CopyFollowConfig {
    /// Only trades from these wallets are ever considered.
    pub followed_wallets: Vec<String>,
    pub sizing: CopySizing,
    /// Hard bounds on the USDC stake of ONE mirrored trade.
    pub min_stake_usd: f64,
    pub max_stake_usd: f64,
    /// Never pay more than this for one outcome token (the chase cap).
    /// Leaders moving a thin book must not drag followers into 0.98s.
    pub max_price: f64,
    /// Slippage tolerance over the leader's fill price, in bps.
    pub slippage_tolerance_bps: u32,
    /// Market filters (mirrors the strategy gates).
    pub min_liquidity_usd: f64,
    pub max_spread: f64,
    /// Quotes older than this are not trusted.
    pub quote_max_age_secs: i64,
}

impl CopyFollowConfig {
    /// Validate the whole config once, before any trade is evaluated.
    /// Returns a human-readable reason or `None` when valid.
    pub fn validate(&self) -> Option<&'static str> {
        if self.followed_wallets.is_empty() {
            return Some("copy: at least one followed wallet is required");
        }
        if !self.min_stake_usd.is_finite() || self.min_stake_usd <= 0.0 {
            return Some("copy: min_stake_usd must be positive");
        }
        if !self.max_stake_usd.is_finite() || self.max_stake_usd < self.min_stake_usd {
            return Some("copy: max_stake_usd must be >= min_stake_usd");
        }
        match &self.sizing {
            CopySizing::FixedUsd(s) if !s.is_finite() || *s <= 0.0 => {
                return Some("copy: fixed stake must be positive")
            }
            CopySizing::MirrorLeader { multiplier }
                if !multiplier.is_finite() || *multiplier <= 0.0 =>
            {
                return Some("copy: mirror multiplier must be positive")
            }
            _ => {}
        }
        if !(0.0..=1.0).contains(&self.max_price) {
            return Some("copy: max_price must be within (0, 1]");
        }
        if self.min_liquidity_usd.is_nan() || self.min_liquidity_usd < 0.0 {
            return Some("copy: min_liquidity_usd must be >= 0");
        }
        if self.max_spread.is_nan() || self.max_spread < 0.0 {
            return Some("copy: max_spread must be >= 0");
        }
        None
    }
}

/// One observed leader trade (ingestion normalizes venue activity rows
/// into this shape before the engine ever sees them).
#[derive(Debug, Clone, PartialEq)]
pub struct LeaderTrade {
    /// Venue trade id — the dedup key.
    pub trade_id: String,
    /// The leader's proxy wallet.
    pub wallet: String,
    /// Market condition id.
    pub condition_id: String,
    /// CTF token id of the outcome traded.
    pub token_id: String,
    /// Human outcome label ("Yes" / "No" / …).
    pub outcome: String,
    /// Leader bought (true) or sold (false).
    pub is_buy: bool,
    /// Leader's fill price in [0, 1].
    pub price: f64,
    /// Leader's size in outcome tokens.
    pub size_tokens: f64,
    /// Leader's USDC stake (price * size).
    pub stake_usd: f64,
    /// Whether the market is neg-risk (selects the exchange contract).
    pub neg_risk: bool,
    /// Display question (signal metadata only).
    pub question: String,
    /// Market liquidity at ingestion time (USD).
    pub liquidity_usd: f64,
    /// Venue tick size string ("0.01" / "0.001").
    pub tick_size: String,
    /// When the leader traded.
    pub at: DateTime<Utc>,
}

/// Why a leader trade was NOT mirrored. Closed, stable vocabulary — every
/// variant is journaled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopySkipReason {
    /// The trade's wallet is not in the followed set.
    NotFollowedWallet,
    /// This trade id was already evaluated (feed replay / duplicate).
    DuplicateTrade,
    /// v1 mirrors ENTRIES only; leader sells are never implicitly turned
    /// into closes (see module doc).
    LeaderSell,
    /// Market liquidity below `min_liquidity_usd`.
    LowLiquidity,
    /// No usable quote for the outcome.
    NoQuote,
    /// One-sided book (no ask to buy into).
    OneSidedBook,
    /// Corrupt / crossed snapshot.
    CrossedBook,
    /// Spread wider than `max_spread`.
    SpreadTooWide,
    /// Quote older than `quote_max_age_secs`.
    StaleQuote,
    /// The price we would have to pay exceeds `max_price`.
    AboveMaxPrice,
    /// The current ask already exceeds leader price + slippage tolerance —
    /// the move ran away; chasing is refused.
    SlippageExceeded,
    /// Rounded size below one tradeable unit / stake below the floor.
    SizedTooSmall,
    /// Config rejected at evaluation time (defence in depth over the
    /// startup `validate()`).
    InvalidConfig,
}

impl CopySkipReason {
    /// Stable machine-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            CopySkipReason::NotFollowedWallet => "NOT_FOLLOWED_WALLET",
            CopySkipReason::DuplicateTrade => "DUPLICATE_TRADE",
            CopySkipReason::LeaderSell => "LEADER_SELL",
            CopySkipReason::LowLiquidity => "LOW_LIQUIDITY",
            CopySkipReason::NoQuote => "NO_QUOTE",
            CopySkipReason::OneSidedBook => "ONE_SIDED_BOOK",
            CopySkipReason::CrossedBook => "CROSSED_BOOK",
            CopySkipReason::SpreadTooWide => "SPREAD_TOO_WIDE",
            CopySkipReason::StaleQuote => "STALE_QUOTE",
            CopySkipReason::AboveMaxPrice => "ABOVE_MAX_PRICE",
            CopySkipReason::SlippageExceeded => "SLIPPAGE_EXCEEDED",
            CopySkipReason::SizedTooSmall => "SIZED_TOO_SMALL",
            CopySkipReason::InvalidConfig => "INVALID_CONFIG",
        }
    }
}

/// The engine's verdict for one leader trade.
#[derive(Debug, Clone, PartialEq)]
pub enum CopyVerdict {
    /// Mirror this trade; the signal is frozen and carries the copy
    /// strategy label into the pipeline.
    Follow(OrderSignal),
    /// Skip, with the explicit reason.
    Skip(CopySkipReason),
}

/// The copy engine: config + followed set + the seen-trade dedup ring.
/// One engine per tenant runtime (server owns construction).
#[derive(Debug)]
pub struct CopyEngine {
    config: CopyFollowConfig,
    followed: HashSet<String>,
    /// Dedup ring: FIFO eviction order + membership set.
    seen_order: VecDeque<String>,
    seen: HashSet<String>,
}

impl CopyEngine {
    /// Build an engine. Invalid configs are rejected at construction.
    pub fn new(config: CopyFollowConfig) -> Result<Self, &'static str> {
        if let Some(reason) = config.validate() {
            return Err(reason);
        }
        let followed: HashSet<String> = config
            .followed_wallets
            .iter()
            .map(|w| w.to_ascii_lowercase())
            .collect();
        Ok(Self {
            config,
            followed,
            seen_order: VecDeque::new(),
            seen: HashSet::new(),
        })
    }

    /// The active configuration (read-only: changing sizing mid-flight is
    /// a server operation that re-builds the engine, so in-flight dedup
    /// state is never silently reinterpreted).
    pub fn config(&self) -> &CopyFollowConfig {
        &self.config
    }

    /// Is this wallet followed?
    pub fn follows(&self, wallet: &str) -> bool {
        self.followed.contains(&wallet.to_ascii_lowercase())
    }

    /// Evaluate one leader trade against the CURRENT quote and either
    /// return a frozen [`OrderSignal`] or the explicit skip reason.
    /// Pure with respect to money: this function commits nothing; the
    /// pipeline re-runs every gate (market, quote, exposure, risk, OMS)
    /// before anything is signed.
    pub fn evaluate(
        &mut self,
        trade: &LeaderTrade,
        quote: Option<&Quote>,
        now: DateTime<Utc>,
        mode: ExecutionMode,
    ) -> CopyVerdict {
        // Dedup FIRST: a replayed trade must not re-consume any gate.
        if self.seen.contains(&trade.trade_id) {
            return CopyVerdict::Skip(CopySkipReason::DuplicateTrade);
        }
        self.mark_seen(&trade.trade_id);

        if !self.follows(&trade.wallet) {
            return CopyVerdict::Skip(CopySkipReason::NotFollowedWallet);
        }
        if !trade.is_buy {
            return CopyVerdict::Skip(CopySkipReason::LeaderSell);
        }
        if trade.liquidity_usd < self.config.min_liquidity_usd {
            return CopyVerdict::Skip(CopySkipReason::LowLiquidity);
        }
        let quote = match quote {
            Some(q) => q,
            None => return CopyVerdict::Skip(CopySkipReason::NoQuote),
        };
        if quote.best_ask.is_nan() || quote.best_bid.is_nan() {
            return CopyVerdict::Skip(CopySkipReason::NoQuote);
        }
        if quote.best_ask <= 0.0 {
            return CopyVerdict::Skip(CopySkipReason::OneSidedBook);
        }
        if quote.best_ask < quote.best_bid {
            return CopyVerdict::Skip(CopySkipReason::CrossedBook);
        }
        if quote.spread() > self.config.max_spread {
            return CopyVerdict::Skip(CopySkipReason::SpreadTooWide);
        }
        if let Some(age) = quote.age_secs(now) {
            if age > self.config.quote_max_age_secs {
                return CopyVerdict::Skip(CopySkipReason::StaleQuote);
            }
        }

        // Slippage: our limit may exceed the leader's fill by the
        // tolerance, never more — and never past the chase cap.
        let tolerance = f64::from(self.config.slippage_tolerance_bps) / 10_000.0;
        let worst_acceptable = trade.price * (1.0 + tolerance);
        if quote.best_ask > worst_acceptable {
            return CopyVerdict::Skip(CopySkipReason::SlippageExceeded);
        }
        if quote.best_ask > self.config.max_price {
            return CopyVerdict::Skip(CopySkipReason::AboveMaxPrice);
        }

        // Sizing: fixed or mirrored, always clamped to the tenant bounds.
        let raw_stake = match &self.config.sizing {
            CopySizing::FixedUsd(s) => *s,
            CopySizing::MirrorLeader { multiplier } => trade.stake_usd * multiplier,
        };
        let stake_usd = raw_stake
            .max(self.config.min_stake_usd)
            .min(self.config.max_stake_usd);
        if !stake_usd.is_finite() || stake_usd <= 0.0 {
            return CopyVerdict::Skip(CopySkipReason::InvalidConfig);
        }
        let size_tokens = round_size(stake_usd / quote.best_ask);
        if size_tokens <= 0.0 || size_tokens * quote.best_ask < self.config.min_stake_usd {
            return CopyVerdict::Skip(CopySkipReason::SizedTooSmall);
        }

        let decision = OrderDecision {
            token_id: trade.token_id.clone(),
            outcome: trade.outcome.clone(),
            is_buy: true,
            size_tokens,
            limit_price: quote.best_ask,
            stake_usd,
            condition_id: trade.condition_id.clone(),
            neg_risk: trade.neg_risk,
            reason: format!(
                "copy: mirrored {} (leader {} @ {:.4}, ask {:.4})",
                trade.trade_id, trade.wallet, trade.price, quote.best_ask
            ),
        };
        // Copy entries are limit GTC at our price bound: the pipeline's
        // GTD/expiry policy still applies downstream.
        let signal = OrderSignal::new(
            decision,
            COPY_STRATEGY,
            mode,
            "GTC",
            &trade.tick_size,
            0,
            &trade.question,
            now,
        );
        CopyVerdict::Follow(signal)
    }

    fn mark_seen(&mut self, trade_id: &str) {
        if self.seen.len() >= SEEN_TRADE_CAP {
            if let Some(evicted) = self.seen_order.pop_front() {
                self.seen.remove(&evicted);
            }
        }
        self.seen.insert(trade_id.to_owned());
        self.seen_order.push_back(trade_id.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn cfg() -> CopyFollowConfig {
        CopyFollowConfig {
            followed_wallets: vec!["0xLEADER".into()],
            sizing: CopySizing::MirrorLeader { multiplier: 1.0 },
            min_stake_usd: 5.0,
            max_stake_usd: 100.0,
            max_price: 0.90,
            slippage_tolerance_bps: 200, // 2%
            min_liquidity_usd: 1_000.0,
            max_spread: 0.05,
            quote_max_age_secs: 30,
        }
    }

    fn trade(id: &str, price: f64, stake: f64, is_buy: bool) -> LeaderTrade {
        LeaderTrade {
            trade_id: id.into(),
            wallet: "0xleader".into(),
            condition_id: "0xcond".into(),
            token_id: "tok".into(),
            outcome: "Yes".into(),
            is_buy,
            price,
            size_tokens: stake / price,
            stake_usd: stake,
            neg_risk: false,
            question: "Will it happen?".into(),
            liquidity_usd: 50_000.0,
            tick_size: "0.01".into(),
            at: Utc::now(),
        }
    }

    fn fresh_quote(ask: f64, now: DateTime<Utc>) -> Quote {
        Quote::observed(ask - 0.02, ask, now)
    }

    #[test]
    fn config_validation_fails_closed() {
        assert!(CopyEngine::new(cfg()).is_ok());
        let mut bad = cfg();
        bad.followed_wallets.clear();
        assert!(CopyEngine::new(bad).is_err());
        let mut bad = cfg();
        bad.max_stake_usd = 1.0; // below min
        assert!(CopyEngine::new(bad).is_err());
        let mut bad = cfg();
        bad.max_price = 1.5;
        assert!(CopyEngine::new(bad).is_err());
    }

    #[test]
    fn mirrors_a_healthy_buy_into_a_frozen_signal() {
        let now = Utc::now();
        let mut engine = CopyEngine::new(cfg()).unwrap();
        let verdict = engine.evaluate(
            &trade("t1", 0.50, 40.0, true),
            Some(&fresh_quote(0.51, now)),
            now,
            ExecutionMode::Paper,
        );
        let signal = match verdict {
            CopyVerdict::Follow(s) => s,
            other => panic!("expected Follow, got {other:?}"),
        };
        assert_eq!(signal.strategy, COPY_STRATEGY);
        assert!(signal.decision.is_buy);
        assert!((signal.decision.stake_usd - 40.0).abs() < 1e-9);
        // size = 40 / 0.51 = 78.43 (rounded to cents)
        assert!((signal.decision.size_tokens - 78.43).abs() < 0.01);
        assert!(signal.decision.reason.starts_with("copy:"));
        assert!(signal.signal_id.starts_with("psig_"));
    }

    #[test]
    fn wallet_matching_is_case_insensitive_and_strict() {
        let now = Utc::now();
        let mut engine = CopyEngine::new(cfg()).unwrap();
        let mut t = trade("t2", 0.5, 40.0, true);
        t.wallet = "0xLeader".into(); // different case -> still followed
        assert!(matches!(
            engine.evaluate(&t, Some(&fresh_quote(0.51, now)), now, ExecutionMode::Paper),
            CopyVerdict::Follow(_)
        ));
        let mut foreign = trade("t3", 0.5, 40.0, true);
        foreign.wallet = "0xstranger".into();
        match engine.evaluate(&foreign, Some(&fresh_quote(0.51, now)), now, ExecutionMode::Paper) {
            CopyVerdict::Skip(r) => assert_eq!(r, CopySkipReason::NotFollowedWallet),
            other => panic!("expected skip, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_trade_ids_are_never_mirrored_twice() {
        let now = Utc::now();
        let mut engine = CopyEngine::new(cfg()).unwrap();
        let t = trade("t9", 0.5, 40.0, true);
        assert!(matches!(
            engine.evaluate(&t, Some(&fresh_quote(0.51, now)), now, ExecutionMode::Paper),
            CopyVerdict::Follow(_)
        ));
        match engine.evaluate(&t, Some(&fresh_quote(0.51, now)), now, ExecutionMode::Paper) {
            CopyVerdict::Skip(r) => assert_eq!(r, CopySkipReason::DuplicateTrade),
            other => panic!("expected dedup skip, got {other:?}"),
        }
    }

    #[test]
    fn leader_sells_are_explicitly_skipped() {
        let now = Utc::now();
        let mut engine = CopyEngine::new(cfg()).unwrap();
        match engine.evaluate(
            &trade("t4", 0.6, 40.0, false),
            Some(&fresh_quote(0.61, now)),
            now,
            ExecutionMode::Paper,
        ) {
            CopyVerdict::Skip(r) => assert_eq!(r, CopySkipReason::LeaderSell),
            other => panic!("expected LeaderSell, got {other:?}"),
        }
    }

    #[test]
    fn slippage_and_chase_caps_refuse_runaway_prices() {
        let now = Utc::now();
        let mut engine = CopyEngine::new(cfg()).unwrap();
        // Leader filled 0.50; ask ran to 0.60 (20% away > 2% tolerance).
        match engine.evaluate(
            &trade("t5", 0.50, 40.0, true),
            Some(&fresh_quote(0.60, now)),
            now,
            ExecutionMode::Paper,
        ) {
            CopyVerdict::Skip(r) => assert_eq!(r, CopySkipReason::SlippageExceeded),
            other => panic!("expected slippage skip, got {other:?}"),
        }
        // Ask within tolerance but above the 0.90 chase cap.
        let mut engine = CopyEngine::new(cfg()).unwrap();
        match engine.evaluate(
            &trade("t6", 0.89, 40.0, true),
            Some(&fresh_quote(0.905, now)),
            now,
            ExecutionMode::Paper,
        ) {
            CopyVerdict::Skip(r) => assert_eq!(r, CopySkipReason::AboveMaxPrice),
            other => panic!("expected chase-cap skip, got {other:?}"),
        }
    }

    #[test]
    fn stake_clamps_to_tenant_bounds() {
        let now = Utc::now();
        let mut engine = CopyEngine::new(cfg()).unwrap();
        // Leader staked 1000 -> mirrored stake clamps to max 100.
        let signal = match engine.evaluate(
            &trade("t7", 0.5, 1_000.0, true),
            Some(&fresh_quote(0.51, now)),
            now,
            ExecutionMode::Paper,
        ) {
            CopyVerdict::Follow(s) => s,
            other => panic!("expected Follow, got {other:?}"),
        };
        assert!((signal.decision.stake_usd - 100.0).abs() < 1e-9);
    }

    #[test]
    fn market_filters_gate_thin_and_stale_books() {
        let now = Utc::now();
        let mut engine = CopyEngine::new(cfg()).unwrap();
        let mut thin = trade("t8", 0.5, 40.0, true);
        thin.liquidity_usd = 10.0;
        match engine.evaluate(&thin, Some(&fresh_quote(0.51, now)), now, ExecutionMode::Paper) {
            CopyVerdict::Skip(r) => assert_eq!(r, CopySkipReason::LowLiquidity),
            other => panic!("expected liquidity skip, got {other:?}"),
        }
        // Stale quote.
        let stale = Quote::observed(0.49, 0.51, now - chrono::Duration::seconds(120));
        let mut engine = CopyEngine::new(cfg()).unwrap();
        match engine.evaluate(
            &trade("t10", 0.5, 40.0, true),
            Some(&stale),
            now,
            ExecutionMode::Paper,
        ) {
            CopyVerdict::Skip(r) => assert_eq!(r, CopySkipReason::StaleQuote),
            other => panic!("expected stale skip, got {other:?}"),
        }
        // Wide spread.
        let wide = Quote::observed(0.40, 0.51, now);
        let mut engine = CopyEngine::new(cfg()).unwrap();
        match engine.evaluate(
            &trade("t11", 0.5, 40.0, true),
            Some(&wide),
            now,
            ExecutionMode::Paper,
        ) {
            CopyVerdict::Skip(r) => assert_eq!(r.as_str(), "SPREAD_TOO_WIDE"),
            other => panic!("expected spread skip, got {other:?}"),
        }
        // No quote at all.
        let mut engine = CopyEngine::new(cfg()).unwrap();
        match engine.evaluate(&trade("t12", 0.5, 40.0, true), None, now, ExecutionMode::Paper) {
            CopyVerdict::Skip(r) => assert_eq!(r, CopySkipReason::NoQuote),
            other => panic!("expected no-quote skip, got {other:?}"),
        }
    }

    #[test]
    fn fixed_sizing_ignores_leader_stake() {
        let mut config = cfg();
        config.sizing = CopySizing::FixedUsd(25.0);
        let now = Utc::now();
        let mut engine = CopyEngine::new(config).unwrap();
        let signal = match engine.evaluate(
            &trade("t13", 0.5, 999.0, true),
            Some(&fresh_quote(0.50, now)),
            now,
            ExecutionMode::Paper,
        ) {
            CopyVerdict::Follow(s) => s,
            other => panic!("expected Follow, got {other:?}"),
        };
        assert!((signal.decision.stake_usd - 25.0).abs() < 1e-9);
    }

    #[test]
    fn seen_ring_is_bounded() {
        let now = Utc::now();
        let mut engine = CopyEngine::new(cfg()).unwrap();
        for i in 0..(SEEN_TRADE_CAP + 50) {
            let verdict = engine.evaluate(
                &trade(&format!("bulk-{i}"), 0.5, 40.0, true),
                Some(&fresh_quote(0.51, now)),
                now,
                ExecutionMode::Paper,
            );
            assert!(matches!(verdict, CopyVerdict::Follow(_)));
        }
        assert!(engine.seen.len() <= SEEN_TRADE_CAP);
        // The earliest id was evicted and can be re-seen (bounded memory
        // beats an unbounded set; replay protection for RECENT ids holds).
        assert!(!engine.seen.contains("bulk-0"));
        assert!(engine.seen.contains(&format!("bulk-{}", SEEN_TRADE_CAP + 49)));
    }
}
