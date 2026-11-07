//! Copy-follow end-to-end: leader trade → engine verdict → the SAME staged
//! pipeline as a strategy entry.
//!
//! These tests exist because the copy engine (`copy.rs`) and the leader feed
//! (`copy_feed.rs`) previously had **no production call site**: both compiled,
//! both were unit-tested, and neither was ever reached by the running bot. Here
//! the real `PolyBot::copy_poll_once` is driven against a mock public data API
//! (`/data-api/trades`), the mock Gamma catalogue and the mock CLOB book, and
//! the resulting signals are asserted to land in the OMS + journal exactly like
//! strategy signals do.
//!
//! Honesty rules these tests encode:
//! * a mirrored entry is a real pipeline outcome (`FILLED` in paper mode), not
//!   a fabricated fill;
//! * a replayed page never mirrors twice;
//! * a failed feed is an error, never "no trades";
//! * every refusal has an explicit `CopySkipReason` label.

mod common;

use std::sync::Arc;

use common::*;
use serde_json::{json, Value};

use module_polymarket::copy::CopyEngine;
use module_polymarket::copy_feed::CopyFeedClient;
use module_polymarket::orders::PolyStage;
use module_polymarket::store::MemoryPolyStore;
use module_polymarket::PolyBot;

/// Followed leader address used by every fixture row.
const LEADER: &str = "0xAbC0000000000000000000000000000000000001";

fn config_with_copy(venue: &MockVenue) -> bot_core::config::Config {
    let mut cfg = base_config(venue);
    cfg.polymarket.copy_followed_wallets = vec![LEADER.to_string()];
    cfg.polymarket.copy_size_multiplier = 1.0;
    cfg.polymarket.copy_min_stake_usd = 1.0;
    cfg.polymarket.copy_max_stake_usd = 25.0;
    cfg.polymarket.copy_max_price = 0.90;
    cfg.polymarket.copy_slippage_tolerance_bps = 300;
    cfg
}

/// One leader BUY of the fixture market's YES outcome at 0.39.
fn leader_buy(price: f64, size: f64, wallet: &str) -> Value {
    json!({
        "proxy_wallet": wallet,
        "side": "BUY",
        "token_id": YES,
        "condition_id": CONDITION,
        "size": size,
        "price": price,
        "timestamp": 1_700_000_000i64,
        "outcome": "Yes",
        "neg_risk": false,
        "title": "Will market 0x1111 resolve Yes?"
    })
}

async fn engine(state: &bot_core::state::Shared) -> CopyEngine {
    let cfg = state.config_snapshot().await.polymarket.clone();
    PolyBot::copy_engine_from_config(&cfg)
        .expect("copy config is valid")
        .expect("engine built for a followed wallet")
}

#[tokio::test]
async fn a_followed_leader_buy_is_mirrored_through_the_staged_pipeline() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(config_with_copy(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let bot = paper_bot(&state, store.clone()).await;
    let poly = poly_cfg(&state).await;

    venue.set_leader_trades(vec![leader_buy(0.39, 20.0, LEADER)]);
    let feed = CopyFeedClient::with_base_url(venue.data_url());
    let mut eng = engine(&state).await;

    let report = bot.copy_poll_once(&mut eng, &feed, &poly).await;

    // The feed was really polled, for the followed wallet.
    let queries = venue.leader_trade_queries();
    assert_eq!(queries.len(), 1, "{queries:?}");
    assert!(
        queries[0]
            .as_deref()
            .unwrap_or_default()
            .contains("user=0xAbC0000000000000000000000000000000000001"),
        "the poll must ask for the followed wallet: {queries:?}"
    );

    assert_eq!(report.wallets, 1);
    assert_eq!(report.feed_errors, 0);
    assert_eq!(report.rows, 1);
    assert_eq!(report.evaluated, 1);
    assert_eq!(report.signals, 1, "skips: {:?}", report.skips);
    assert_eq!(report.rejected, 0, "{report:?}");

    // Paper mode: the pipeline books a real paper fill (no venue POST).
    assert_eq!(report.reached_venue, 1);
    assert_eq!(venue.posted_orders(), 0);

    let signals = store.signals().await;
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].strategy, "copy", "copy signals are labelled");
    assert_eq!(signals[0].stage, PolyStage::Filled.as_str());

    let orders = oms.list(10).await;
    assert_eq!(orders.len(), 1);
    let pos = state
        .find_open(bot_core::models::BotModule::Polymarket, YES)
        .await
        .expect("a mirrored position exists");
    assert!(pos.qty > 0.0);
    // Sized from the leader's stake (20 * 0.39 = 7.80 USD) at the ask 0.40.
    let expected = module_polymarket::strategy::round_size(7.8 / 0.40);
    assert!(
        (pos.qty - expected).abs() < 1e-9,
        "{} vs {expected}",
        pos.qty
    );
}

#[tokio::test]
async fn re_polling_the_same_page_never_mirrors_twice() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(config_with_copy(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let bot = paper_bot(&state, store.clone()).await;
    let poly = poly_cfg(&state).await;

    venue.set_leader_trades(vec![leader_buy(0.39, 20.0, LEADER)]);
    let feed = CopyFeedClient::with_base_url(venue.data_url());
    let mut eng = engine(&state).await;

    let first = bot.copy_poll_once(&mut eng, &feed, &poly).await;
    assert_eq!(first.signals, 1);
    let second = bot.copy_poll_once(&mut eng, &feed, &poly).await;
    assert_eq!(second.rows, 1, "the row is still in the page");
    assert_eq!(second.evaluated, 0, "but it is not re-evaluated");

    assert_eq!(oms.list(10).await.len(), 1);
    assert_eq!(store.signals().await.len(), 1);
}

#[tokio::test]
async fn a_leader_sell_is_an_explicit_skip_not_an_implicit_close() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(config_with_copy(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let bot = paper_bot(&state, store.clone()).await;
    let poly = poly_cfg(&state).await;

    let mut row = leader_buy(0.60, 10.0, LEADER);
    row["side"] = json!("SELL");
    venue.set_leader_trades(vec![row]);
    let feed = CopyFeedClient::with_base_url(venue.data_url());
    let mut eng = engine(&state).await;

    let report = bot.copy_poll_once(&mut eng, &feed, &poly).await;
    assert_eq!(report.signals, 0);
    assert_eq!(
        report
            .skips
            .iter()
            .find(|(r, _)| *r == "LEADER_SELL")
            .map(|(_, n)| *n),
        Some(1),
        "{:?}",
        report.skips
    );
    assert_eq!(oms.list(10).await.len(), 0);
    assert!(store.signals().await.is_empty());
}

#[tokio::test]
async fn a_wallet_that_is_not_followed_is_never_mirrored() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(config_with_copy(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let bot = paper_bot(&state, store.clone()).await;
    let poly = poly_cfg(&state).await;

    venue.set_leader_trades(vec![leader_buy(
        0.39,
        20.0,
        "0xsomebodythatisnotfollowed000000000000001",
    )]);
    let feed = CopyFeedClient::with_base_url(venue.data_url());
    let mut eng = engine(&state).await;

    let report = bot.copy_poll_once(&mut eng, &feed, &poly).await;
    assert_eq!(report.evaluated, 1);
    assert_eq!(report.signals, 0);
    assert_eq!(
        report
            .skips
            .iter()
            .find(|(r, _)| *r == "NOT_FOLLOWED_WALLET")
            .map(|(_, n)| *n),
        Some(1),
        "{:?}",
        report.skips
    );
    assert_eq!(oms.list(10).await.len(), 0);
}

#[tokio::test]
async fn a_runaway_price_is_refused_by_the_slippage_gate() {
    let venue = mock_venue().await;
    let mut cfg = config_with_copy(&venue);
    cfg.polymarket.copy_slippage_tolerance_bps = 0;
    let (state, oms) = state_with_oms(cfg);
    let store = Arc::new(MemoryPolyStore::new());
    let bot = paper_bot(&state, store.clone()).await;
    let poly = poly_cfg(&state).await;

    // Leader filled at 0.20; our book's best ask is 0.40 — the move ran away.
    venue.set_leader_trades(vec![leader_buy(0.20, 20.0, LEADER)]);
    let feed = CopyFeedClient::with_base_url(venue.data_url());
    let mut eng = engine(&state).await;

    let report = bot.copy_poll_once(&mut eng, &feed, &poly).await;
    assert_eq!(report.signals, 0);
    assert_eq!(
        report
            .skips
            .iter()
            .find(|(r, _)| *r == "SLIPPAGE_EXCEEDED")
            .map(|(_, n)| *n),
        Some(1),
        "{:?}",
        report.skips
    );
    assert_eq!(oms.list(10).await.len(), 0);
}

#[tokio::test]
async fn a_failed_feed_is_an_error_never_no_trades() {
    let venue = mock_venue().await;
    let (state, _oms) = state_with_oms(config_with_copy(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let bot = paper_bot(&state, store.clone()).await;
    let poly = poly_cfg(&state).await;

    venue.fail_leader_trades();
    let feed = CopyFeedClient::with_base_url(venue.data_url());
    let mut eng = engine(&state).await;

    let report = bot.copy_poll_once(&mut eng, &feed, &poly).await;
    assert_eq!(report.feed_errors, 1);
    assert_eq!(report.rows, 0);
    assert_eq!(report.signals, 0);
    assert!(store.signals().await.is_empty());
}

#[tokio::test]
async fn no_followed_wallet_means_no_feed_request_at_all() {
    let venue = mock_venue().await;
    let (state, _oms) = state_with_oms(base_config(&venue));
    let poly = poly_cfg(&state).await;
    assert!(PolyBot::copy_engine_from_config(&poly).unwrap().is_none());
    assert!(venue.leader_trade_queries().is_empty());
}

#[tokio::test]
async fn malformed_rows_are_counted_not_turned_into_orders() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(config_with_copy(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let bot = paper_bot(&state, store.clone()).await;
    let poly = poly_cfg(&state).await;

    venue.set_leader_trades(vec![
        json!({"proxy_wallet": LEADER, "side": "BUY"}), // missing everything else
        leader_buy(0.39, 20.0, LEADER),
    ]);
    let feed = CopyFeedClient::with_base_url(venue.data_url());
    let mut eng = engine(&state).await;

    let report = bot.copy_poll_once(&mut eng, &feed, &poly).await;
    assert_eq!(report.rows, 2);
    assert_eq!(report.skipped_rows, 1);
    assert_eq!(report.evaluated, 1);
    assert_eq!(report.signals, 1);
    assert_eq!(oms.list(10).await.len(), 1);
}
