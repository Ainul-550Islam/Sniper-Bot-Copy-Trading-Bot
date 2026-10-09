//! Live market-data aggregator (GAP-MAP P1 REWRITE — replaces the old
//! static 7-entry fake catalog).
//!
//! ## Honesty contract
//!
//! * Every number served here comes from a REAL provider call made at
//!   `fetched_at`: SOL/USD via the Jupiter quote API (`solana-kit`),
//!   Polymarket markets via the Gamma API (`module-polymarket`).
//! * Fields a provider does not report (24h change, DEX liquidity for the
//!   SOL pair) are ZERO, never invented.
//! * A feed that fails is dropped from the snapshot and reported in
//!   [`MarketFeedSnapshot::feeds`] with the error. If every feed fails and
//!   the cache is empty, the caller gets an empty list and the HTTP layer
//!   answers `503 market_data_unavailable` — the same fail-closed posture
//!   the endpoint had before, except now it lifts automatically the moment
//!   a real feed answers.
//! * Results are cached for `ttl` (single-flight refresh: one in-flight
//!   fetch per expiry, other callers wait for it).

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio::sync::Mutex;

use bot_core::market_data::MarketTicker;
use bot_core::models::{BotModule, PolyMarket, Venue};
use module_polymarket::gamma::{GammaClient, MarketQuery};
use solana_kit::consts::{USDC_MINT, WSOL_MINT};
use solana_kit::jupiter::{Jupiter, QuoteRequest};

/// Cache lifetime for aggregated snapshots (overridable via
/// [`MarketService::with_ttl`]).
pub const DEFAULT_MARKET_CACHE_TTL: Duration = Duration::from_secs(30);

/// Polymarket discovery page size for the tenant markets endpoint.
const POLYMARKET_PAGE_LIMIT: usize = 25;

/// One SOL quoted against USDC (both 6-decimal-friendly mints).
const SOL_QUOTE_LAMPORTS: u64 = 1_000_000_000;

/// Per-feed health row, surfaced to operators.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FeedStatus {
    pub name: &'static str,
    pub ok: bool,
    /// Human-readable error when `ok` is false, empty otherwise.
    pub detail: String,
    pub fetched_at: DateTime<Utc>,
    pub tickers: usize,
}

/// What one aggregation produced.
#[derive(Debug, Clone)]
pub struct MarketFeedSnapshot {
    pub tickers: Vec<MarketTicker>,
    pub fetched_at: DateTime<Utc>,
    pub feeds: Vec<FeedStatus>,
    /// `true` when served from cache without a refresh.
    pub from_cache: bool,
}

struct CacheState {
    snapshot: Option<MarketFeedSnapshot>,
    cached_at: Option<Instant>,
}

/// Aggregates the attached live feeds behind a TTL cache.
///
/// Construction never fails: a provider that cannot be built simply never
/// contributes (and is reported as such), so the endpoint degrades to
/// `unavailable` instead of crashing startup.
pub struct MarketService {
    jupiter: Option<Arc<Jupiter>>,
    gamma: Option<Arc<GammaClient>>,
    ttl: Duration,
    state: Mutex<CacheState>,
}

impl MarketService {
    /// Attach every feed the deployment config can build.
    pub fn from_config(cfg: &bot_core::config::Config) -> Arc<Self> {
        let jupiter = Some(Arc::new(Jupiter::new()));
        let gamma = match GammaClient::new(cfg.polymarket.gamma_url.clone()) {
            Ok(client) => Some(Arc::new(client)),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    url = %cfg.polymarket.gamma_url,
                    "market_service: gamma client unavailable — polymarket feed disabled"
                );
                None
            }
        };
        Arc::new(MarketService {
            jupiter,
            gamma,
            ttl: DEFAULT_MARKET_CACHE_TTL,
            state: Mutex::new(CacheState {
                snapshot: None,
                cached_at: None,
            }),
        })
    }

    pub fn with_ttl(self: Arc<Self>, ttl: Duration) -> Arc<Self> {
        // Rebuild is cheap: the struct only holds client handles.
        Arc::new(MarketService {
            jupiter: self.jupiter.clone(),
            gamma: self.gamma.clone(),
            ttl,
            state: Mutex::new(CacheState {
                snapshot: None,
                cached_at: None,
            }),
        })
    }

    /// The current snapshot: fresh cache, or a single-flight refresh.
    ///
    /// The returned list may be empty — that is the honest "no live feed
    /// answered" state the HTTP layer must translate into 503.
    pub async fn snapshot(&self) -> MarketFeedSnapshot {
        let mut state = self.state.lock().await;
        if let (Some(snapshot), Some(cached_at)) = (&state.snapshot, state.cached_at) {
            if cached_at.elapsed() < self.ttl {
                let mut cached = snapshot.clone();
                cached.from_cache = true;
                return cached;
            }
        }
        // Refresh while holding the lock: concurrent callers wait for this
        // one fetch instead of stampeding the providers.
        let feeds = self.fetch_all().await;
        let tickers: Vec<MarketTicker> = feeds.iter().flat_map(|f| f.tickers.clone()).collect();
        let snapshot = MarketFeedSnapshot {
            tickers,
            fetched_at: Utc::now(),
            feeds: feeds.into_iter().map(|f| f.status).collect(),
            from_cache: false,
        };
        state.snapshot = Some(snapshot.clone());
        state.cached_at = Some(Instant::now());
        snapshot
    }

    /// Health view without triggering a fetch.
    pub async fn last_feed_status(&self) -> Vec<FeedStatus> {
        self.state
            .lock()
            .await
            .snapshot
            .as_ref()
            .map(|s| s.feeds.clone())
            .unwrap_or_default()
    }

    async fn fetch_all(&self) -> Vec<FeedResult> {
        let jupiter = self.jupiter.clone();
        let gamma = self.gamma.clone();
        let (sol, poly) = tokio::join!(
            async move { fetch_sol_feed(jupiter.as_deref()).await },
            async move { fetch_polymarket_feed(gamma.as_deref()).await },
        );
        vec![sol, poly]
    }
}

struct FeedResult {
    status: FeedStatus,
    tickers: Vec<MarketTicker>,
}

/// SOL/USD from a live Jupiter quote of exactly 1 SOL into USDC.
///
/// USDC is 6-decimal: `out_amount` micro-USDC → cents is `out_amount /
/// 10_000`. Volume/liquidity/24h-change are not part of a quote and stay
/// zero rather than being invented.
async fn fetch_sol_feed(jupiter: Option<&Jupiter>) -> FeedResult {
    let now = Utc::now();
    let Some(jupiter) = jupiter else {
        return FeedResult {
            status: FeedStatus {
                name: "jupiter_sol_usd",
                ok: false,
                detail: "jupiter client not attached".into(),
                fetched_at: now,
                tickers: 0,
            },
            tickers: Vec::new(),
        };
    };
    let request = QuoteRequest::new(*WSOL_MINT, *USDC_MINT, SOL_QUOTE_LAMPORTS);
    match jupiter.quote(&request).await {
        Ok(quote) => match quote.out_amount.parse::<u64>() {
            Ok(out_micro_usdc) => {
                let price_usd_cents = out_micro_usdc / 10_000;
                let ticker = MarketTicker::new(
                    "mkt-sol-usdc".into(),
                    "SOL/USDC".into(),
                    "Solana (live Jupiter quote)".into(),
                    Venue::Jupiter,
                    "SOL".into(),
                    "USDC".into(),
                    price_usd_cents,
                    0, // 24h change: not reported by the quote API — never invented
                    0, // volume: same
                    0, // liquidity: same
                    vec![BotModule::Sniper, BotModule::Copy],
                    now,
                );
                FeedResult {
                    status: FeedStatus {
                        name: "jupiter_sol_usd",
                        ok: true,
                        detail: String::new(),
                        fetched_at: now,
                        tickers: 1,
                    },
                    tickers: vec![ticker],
                }
            }
            Err(error) => FeedResult {
                status: FeedStatus {
                    name: "jupiter_sol_usd",
                    ok: false,
                    detail: format!("unparseable out_amount: {error}"),
                    fetched_at: now,
                    tickers: 0,
                },
                tickers: Vec::new(),
            },
        },
        Err(error) => FeedResult {
            status: FeedStatus {
                name: "jupiter_sol_usd",
                ok: false,
                detail: error.to_string(),
                fetched_at: now,
                tickers: 0,
            },
            tickers: Vec::new(),
        },
    }
}

/// Polymarket discovery from the live Gamma API: the top active markets by
/// 24h volume, each as a YES-share ticker priced in probability cents.
async fn fetch_polymarket_feed(gamma: Option<&GammaClient>) -> FeedResult {
    let now = Utc::now();
    let Some(gamma) = gamma else {
        return FeedResult {
            status: FeedStatus {
                name: "polymarket_gamma",
                ok: false,
                detail: "gamma client not attached".into(),
                fetched_at: now,
                tickers: 0,
            },
            tickers: Vec::new(),
        };
    };
    let query = MarketQuery {
        active: Some(true),
        closed: Some(false),
        limit: Some(POLYMARKET_PAGE_LIMIT),
        order: Some("volume24hr".into()),
        ascending: Some(false),
        ..Default::default()
    };
    match gamma.markets(&query).await {
        Ok(markets) => {
            let tickers: Vec<MarketTicker> = markets
                .iter()
                .filter_map(|m| gamma_market_to_ticker(m, now))
                .collect();
            let count = tickers.len();
            FeedResult {
                status: FeedStatus {
                    name: "polymarket_gamma",
                    ok: true,
                    detail: String::new(),
                    fetched_at: now,
                    tickers: count,
                },
                tickers,
            }
        }
        Err(error) => FeedResult {
            status: FeedStatus {
                name: "polymarket_gamma",
                ok: false,
                detail: error.to_string(),
                fetched_at: now,
                tickers: 0,
            },
            tickers: Vec::new(),
        },
    }
}

/// Map one normalized Polymarket market. Markets without an id, a question or
/// a usable YES price are skipped — serving them would mean showing tenants a
/// row we cannot stand behind.
fn gamma_market_to_ticker(market: &PolyMarket, now: DateTime<Utc>) -> Option<MarketTicker> {
    let id = non_blank(&market.condition_id).or_else(|| non_blank(&market.slug))?;
    let name = non_blank(&market.question).unwrap_or_else(|| id.clone());
    // The YES share price in [0, 1] → probability cents. A market without a
    // YES outcome (or with a non-finite price) is dropped, never priced by guess.
    let yes = market
        .outcome("Yes")
        .or_else(|| market.outcomes.first())?;
    let yes_price = yes.price;
    if !yes_price.is_finite() || !(0.0..=1.0).contains(&yes_price) {
        return None;
    }
    let price_usd_cents = (yes_price * 100.0).round() as u64;
    let volume_usd_cents = (market.volume.max(0.0) * 100.0) as u64;
    let liquidity_usd_cents = (market.liquidity.max(0.0) * 100.0) as u64;
    let symbol = non_blank(&market.slug)
        .unwrap_or_else(|| id.clone())
        .chars()
        .take(40)
        .collect::<String>();
    Some(MarketTicker::new(
        id,
        symbol,
        name,
        Venue::PolymarketClob,
        "YES".into(),
        "USDC".into(),
        price_usd_cents,
        0, // Gamma discovery reports no 24h change — never invented
        volume_usd_cents,
        liquidity_usd_cents,
        vec![BotModule::Polymarket],
        now,
    ))
}

/// Trimmed copy of a text field, or `None` when it is blank.
fn non_blank(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::models::PolyOutcome;

    fn market(condition_id: &str, slug: &str, yes_price: f64, volume: f64, liquidity: f64) -> PolyMarket {
        PolyMarket {
            condition_id: condition_id.into(),
            question: "Will it work?".into(),
            slug: slug.into(),
            neg_risk: false,
            active: true,
            closed: false,
            accepting_orders: true,
            end_date: None,
            volume,
            liquidity,
            outcomes: vec![
                PolyOutcome {
                    outcome: "Yes".into(),
                    token_id: "1".into(),
                    price: yes_price,
                    winner: None,
                },
                PolyOutcome {
                    outcome: "No".into(),
                    token_id: "2".into(),
                    price: 1.0 - yes_price,
                    winner: None,
                },
            ],
        }
    }

    /// A service with NO feeds attached must answer empty (the 503 state),
    /// never a fabricated catalog.
    #[tokio::test]
    async fn no_feeds_means_empty_snapshot() {
        let service = MarketService {
            jupiter: None,
            gamma: None,
            ttl: DEFAULT_MARKET_CACHE_TTL,
            state: Mutex::new(CacheState {
                snapshot: None,
                cached_at: None,
            }),
        };
        let snapshot = service.snapshot().await;
        assert!(snapshot.tickers.is_empty());
        assert!(!snapshot.from_cache);
        assert_eq!(snapshot.feeds.len(), 2);
        assert!(snapshot.feeds.iter().all(|f| !f.ok));
    }

    #[tokio::test]
    async fn second_read_within_ttl_is_served_from_cache() {
        let service = MarketService {
            jupiter: None,
            gamma: None,
            ttl: Duration::from_secs(60),
            state: Mutex::new(CacheState {
                snapshot: None,
                cached_at: None,
            }),
        };
        let first = service.snapshot().await;
        assert!(!first.from_cache);
        let second = service.snapshot().await;
        assert!(second.from_cache);
        // Feed statuses survive the cache round trip.
        assert_eq!(second.feeds.len(), 2);
    }

    #[tokio::test]
    async fn expired_cache_triggers_a_refresh() {
        let service = MarketService {
            jupiter: None,
            gamma: None,
            ttl: Duration::from_millis(1),
            state: Mutex::new(CacheState {
                snapshot: None,
                cached_at: None,
            }),
        };
        let _ = service.snapshot().await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        let refreshed = service.snapshot().await;
        assert!(!refreshed.from_cache);
    }

    #[test]
    fn gamma_rows_without_id_or_price_are_dropped() {
        let now = Utc::now();
        let no_id = market("", "", 0.5, 0.0, 0.0);
        assert!(gamma_market_to_ticker(&no_id, now).is_none());

        let mut no_outcomes = market("0xdef", "no-outcomes", 0.5, 0.0, 0.0);
        no_outcomes.outcomes.clear();
        assert!(gamma_market_to_ticker(&no_outcomes, now).is_none());

        let nan_price = market("0xnan", "nan-price", f64::NAN, 0.0, 0.0);
        assert!(gamma_market_to_ticker(&nan_price, now).is_none());

        let good = market("0xabc", "will-it-work", 0.62, 1234.5, 500.0);
        let ticker = gamma_market_to_ticker(&good, now).expect("valid row maps");
        assert_eq!(ticker.price_usd_cents, 62);
        assert_eq!(ticker.volume_24h_usd_cents, 123_450);
        assert_eq!(ticker.liquidity_usd_cents, 50_000);
        assert_eq!(ticker.venue, Venue::PolymarketClob);
    }
}
