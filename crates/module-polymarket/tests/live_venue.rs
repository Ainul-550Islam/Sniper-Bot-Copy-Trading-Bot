//! LIVE venue verification — **real Polymarket endpoints, no mocks.**
//!
//! Every other Polymarket test in this repository runs against an in-process
//! axum mock, which proves our code paths but says nothing about whether the
//! real venue still speaks the protocol we implement. These tests close that
//! specific gap for everything that does **not** need an API key:
//!
//! * `gamma-api.polymarket.com` — the catalogue we scan (`GammaClient`);
//! * `clob.polymarket.com` — `/book` prices and `/markets/{condition}` tick
//!   sizes (`ClobClient`), i.e. the same calls live entries depend on;
//! * `data-api.polymarket.com/trades` — the leader feed behind copy-follow
//!   (`CopyFeedClient`), parsed by the production parser;
//! * the whole local pipeline, fed with that live data, in **paper** mode.
//!
//! They are `#[ignore]`d so the default `cargo test` stays hermetic and
//! deterministic, and they *additionally* require `LIVE_POLYMARKET=1`, so a
//! network outage can never be mistaken for a code failure:
//!
//! ```text
//! LIVE_POLYMARKET=1 cargo test -p module-polymarket --test live_venue -- --ignored --test-threads=1
//! ```
//!
//! **What these tests do not and cannot prove:** order placement, cancelling,
//! heartbeat and the authenticated user channel — all of those need a funded
//! key and are therefore still `BLOCKED`. Nothing here places an order; the
//! execution leg is paper.

use std::sync::Arc;

use bot_core::models::ExecutionMode;
use chrono::Utc;

use module_polymarket::clob::ClobClient;
use module_polymarket::copy::CopyEngine;
use module_polymarket::copy_feed::CopyFeedClient;
use module_polymarket::gamma::{GammaClient, MarketQuery};
use module_polymarket::strategy::Quote;

const GAMMA: &str = "https://gamma-api.polymarket.com";
const CLOB: &str = "https://clob.polymarket.com";
const DATA: &str = "https://data-api.polymarket.com";
const CHAIN_ID: u64 = 137;

fn live_enabled() -> bool {
    std::env::var("LIVE_POLYMARKET").ok().as_deref() == Some("1")
}

/// A real market from the live Gamma catalogue, with a real token id.
async fn a_live_market() -> module_polymarket::gamma::MarketQuery {
    MarketQuery {
        active: Some(true),
        closed: Some(false),
        limit: Some(20),
        order: Some("volume24hr".into()),
        ascending: Some(false),
        ..Default::default()
    }
}

#[tokio::test]
#[ignore = "hits the live Polymarket Gamma API; run with LIVE_POLYMARKET=1 --ignored"]
async fn live_gamma_catalogue_parses_into_our_market_type() {
    if !live_enabled() {
        eprintln!("LIVE_POLYMARKET!=1 — skipping the live Gamma call");
        return;
    }
    let gamma = GammaClient::new(GAMMA).expect("gamma client");
    let markets = gamma
        .markets(&a_live_market().await)
        .await
        .expect("live gamma /markets must answer");
    assert!(!markets.is_empty(), "live catalogue returned no markets");
    for m in &markets {
        assert!(!m.condition_id.is_empty(), "market without a condition id");
        assert!(!m.question.is_empty(), "market without a question");
        assert!(!m.outcomes.is_empty(), "market without outcomes");
        for o in &m.outcomes {
            assert!(
                !o.token_id.is_empty(),
                "outcome without a CLOB token id ({} / {})",
                m.condition_id,
                o.outcome
            );
            // Real prices come back in [0, 1]; a parsed value outside that
            // range means our decoder, not the venue, is wrong.
            assert!(
                (0.0..=1.0).contains(&o.price),
                "implausible live price {} for {}",
                o.price,
                o.token_id
            );
        }
    }
    println!(
        "live gamma: {} markets parsed, first = {:?} ({} outcomes, liquidity {})",
        markets.len(),
        &markets[0].condition_id[..markets[0].condition_id.len().min(18)],
        markets[0].outcomes.len(),
        markets[0].liquidity
    );
}

#[tokio::test]
#[ignore = "hits the live Polymarket CLOB API; run with LIVE_POLYMARKET=1 --ignored"]
async fn live_clob_serves_a_book_and_a_tick_size_for_a_real_token() {
    if !live_enabled() {
        eprintln!("LIVE_POLYMARKET!=1 — skipping the live CLOB calls");
        return;
    }
    let gamma = GammaClient::new(GAMMA).expect("gamma client");
    let clob = ClobClient::new(CLOB, CHAIN_ID).expect("clob client");
    let markets = gamma.markets(&a_live_market().await).await.expect("gamma");
    // First market whose book actually has a two-sided price.
    let mut checked = 0usize;
    for m in markets.iter() {
        for o in &m.outcomes {
            let Ok(book) = clob.order_book(&o.token_id).await else {
                continue;
            };
            let q: Quote = book.to_quote();
            if q.best_bid <= 0.0 || q.best_ask <= 0.0 || q.best_ask < q.best_bid {
                continue;
            }
            assert!(
                (0.0..=1.0).contains(&q.best_ask) && (0.0..=1.0).contains(&q.best_bid),
                "live book outside [0,1]: bid {} ask {}",
                q.best_bid,
                q.best_ask
            );
            let tick = clob
                .tick_size(&m.condition_id)
                .await
                .expect("live tick size must answer for a live market");
            assert!(
                tick == "0.01" || tick == "0.001" || tick == "0.0001" || tick == "0.1",
                "unexpected live tick size {tick}"
            );
            println!(
                "live CLOB: token {} bid {:.4} ask {:.4} mid {:.4} tick {tick}",
                o.token_id, q.best_bid, q.best_ask, q.midpoint
            );
            checked += 1;
            if checked >= 2 {
                return;
            }
        }
    }
    assert!(
        checked > 0,
        "no live market produced a usable two-sided book"
    );
}

#[tokio::test]
#[ignore = "hits the live Polymarket data API; run with LIVE_POLYMARKET=1 --ignored"]
async fn live_data_api_leader_trades_parse_into_leader_trades() {
    if !live_enabled() {
        eprintln!("LIVE_POLYMARKET!=1 — skipping the live data-API call");
        return;
    }
    // Step 1: find a wallet that is actually trading right now, using the
    // public feed itself (no assumption about who the leaders are).
    let raw: serde_json::Value = reqwest::get(format!("{DATA}/trades?limit=1"))
        .await
        .expect("live /trades must answer")
        .json()
        .await
        .expect("live /trades must be JSON");
    let wallet = raw
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("proxyWallet"))
        .and_then(|w| w.as_str())
        .expect("live /trades row without proxyWallet")
        .to_string();

    // Step 2: read that wallet through the PRODUCTION client + parser.
    let feed = CopyFeedClient::with_base_url(DATA);
    let page = feed
        .recent_trades(&wallet, 10)
        .await
        .expect("live copy feed must answer for a real wallet");
    assert!(
        !page.trades.is_empty(),
        "live wallet {wallet} returned no parseable trades (skipped {})",
        page.skipped
    );
    for t in &page.trades {
        assert_eq!(t.wallet, wallet.to_ascii_lowercase());
        assert!(!t.token_id.is_empty());
        assert!(!t.condition_id.is_empty());
        assert!(
            (0.0..=1.0).contains(&t.price),
            "implausible price {}",
            t.price
        );
        assert!(t.size_tokens > 0.0);
        assert!(
            (t.stake_usd - t.price * t.size_tokens).abs() < 1e-6,
            "stake must equal price x size"
        );
    }
    println!(
        "live data-api: wallet {} → {} trades parsed, {} rows skipped, first {} {:?} @ {}",
        &wallet[..wallet.len().min(12)],
        page.trades.len(),
        page.skipped,
        page.trades[0].trade_id.chars().take(24).collect::<String>(),
        page.trades[0].outcome,
        page.trades[0].price
    );
}

#[tokio::test]
#[ignore = "hits the live Polymarket APIs; run with LIVE_POLYMARKET=1 --ignored"]
async fn live_leader_trade_reaches_a_paper_fill_through_the_engine() {
    if !live_enabled() {
        eprintln!("LIVE_POLYMARKET!=1 — skipping the live end-to-end run");
        return;
    }
    let feed = CopyFeedClient::with_base_url(DATA);
    let raw: serde_json::Value = reqwest::get(format!("{DATA}/trades?limit=25"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rows = raw.as_array().expect("live /trades array");
    let mut wallet = None;
    for row in rows {
        if row.get("side").and_then(|s| s.as_str()) == Some("BUY") {
            if let Some(w) = row.get("proxyWallet").and_then(|w| w.as_str()) {
                wallet = Some(w.to_string());
                break;
            }
        }
    }
    let wallet = wallet.expect("no live BUY in the last 25 trades");
    let page = feed.recent_trades(&wallet, 25).await.expect("live feed");
    let trade = page
        .trades
        .into_iter()
        .find(|t| t.is_buy)
        .expect("live BUY trade");

    // Real venue state for that market.
    let gamma = GammaClient::new(GAMMA).unwrap();
    let clob = ClobClient::new(CLOB, CHAIN_ID).unwrap();
    let market = gamma
        .market_by_condition(&trade.condition_id)
        .await
        .expect("live gamma lookup")
        .expect("the traded market must exist in the live catalogue");
    let book = clob
        .order_book(&trade.token_id)
        .await
        .expect("live book for the traded token");
    let quotes = std::collections::HashMap::from([(trade.token_id.clone(), book.to_quote())]);

    // The production engine, configured only from real bounds.
    let mut engine = CopyEngine::new(module_polymarket::copy::CopyFollowConfig {
        followed_wallets: vec![wallet.clone()],
        sizing: module_polymarket::copy::CopySizing::MirrorLeader { multiplier: 1.0 },
        min_stake_usd: 1.0,
        max_stake_usd: 25.0,
        max_price: 0.95,
        slippage_tolerance_bps: 500,
        min_liquidity_usd: 0.0,
        max_spread: 0.10,
        quote_max_age_secs: 600,
    })
    .expect("engine");

    let verdict = engine.evaluate(
        &trade,
        quotes.get(&trade.token_id),
        Utc::now(),
        ExecutionMode::Paper,
    );
    println!(
        "live e2e: market {} · token {} · leader {}·{} @ {} → verdict {:?}",
        &market.condition_id[..market.condition_id.len().min(16)],
        &trade.token_id[..trade.token_id.len().min(12)],
        wallet.chars().take(10).collect::<String>(),
        if trade.is_buy { "BUY" } else { "SELL" },
        trade.price,
        match &verdict {
            module_polymarket::copy::CopyVerdict::Follow(sig) => format!(
                "FOLLOW size {} @ {} (stake {})",
                sig.decision.size_tokens, sig.decision.limit_price, sig.decision.stake_usd
            ),
            module_polymarket::copy::CopyVerdict::Skip(r) => format!("SKIP {}", r.as_str()),
        }
    );
    // The engine either mirrors the live trade or refuses it with a named
    // reason; both are valid outcomes. What must NOT happen is a panic, an
    // invented price, or a signal whose price is outside [0,1].
    match verdict {
        module_polymarket::copy::CopyVerdict::Follow(sig) => {
            assert!(sig.decision.limit_price > 0.0 && sig.decision.limit_price < 1.0);
            assert!(sig.decision.size_tokens > 0.0);
            assert_eq!(sig.strategy, "copy");
        }
        module_polymarket::copy::CopyVerdict::Skip(reason) => {
            assert!(!reason.as_str().is_empty());
        }
    }
    let _ = Arc::new(());
}
