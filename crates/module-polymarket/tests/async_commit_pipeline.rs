//! Async commit pipeline — the acceptance→settlement gap (PROMPT
//! 4/10 §D).
//!
//! Against the mock CLOB (real HTTP, real client code paths):
//!
//! * a `matched` acceptance WITHOUT settlement hashes lands in the
//!   async registry and is resolved later from `GET /data/trades`
//!   (`CONFIRMED` + hash ⇒ a backfilled fill; the hash is the
//!   venue's own value, never synthesized);
//! * a `delayed` acceptance is PENDING — zero amounts, no ids, no
//!   fill booked — and stays pending while the venue delays;
//! * an order the venue no longer knows is dropped from the registry
//!   (`OrderGoneOnVenue`);
//! * a terminal-`FAILED` trade is reported as a failure and never
//!   becomes a fill;
//! * a settlement wait that times out reports `StillSettling` —
//!   "keep waiting", never a failure verdict (official client
//!   semantics);
//! * an end-to-end live submission through the EXISTING pipeline
//!   registers its own async acceptance when the venue answers
//!   matched-without-hashes.

mod common;

use std::sync::Arc;
use std::time::Duration;

use bot_core::models::ExecutionMode;
use common::*;
use module_polymarket::async_commit::{AcceptanceStatus, AsyncOrderAcceptance, CommitState};
use module_polymarket::backfill::{
    poll_once, AsyncPendingRegistry, BackfillVerdict, PendingAcceptance,
};
use module_polymarket::clob::ClobClient;
use module_polymarket::orders::PolyStage;
use module_polymarket::store::MemoryPolyStore;
use module_polymarket::trade_resolution::wait_for_settlement;
use serde_json::json;

fn authed_client(venue: &MockVenue) -> ClobClient {
    ClobClient::new(venue.clob_url(), 137)
        .expect("client")
        .with_auth(test_address(), test_api_key())
}

fn acceptance(
    status: &str,
    order_id: &str,
    trade_ids: &[&str],
    hashes: &[&str],
) -> AsyncOrderAcceptance {
    AsyncOrderAcceptance {
        order_id: Some(order_id.to_string()),
        status: AcceptanceStatus::parse(Some(status)),
        success: Some(true),
        error_msg: None,
        making_amount: Some(0),
        taking_amount: Some(0),
        trade_ids: trade_ids.iter().map(|s| s.to_string()).collect(),
        transactions_hashes: hashes.iter().map(|s| s.to_string()).collect(),
    }
}

fn pending_of(a: &AsyncOrderAcceptance) -> PendingAcceptance {
    PendingAcceptance::from_acceptance(a, chrono::Utc::now(), chrono::Duration::seconds(900))
        .expect("pending record")
}

#[tokio::test]
async fn matched_without_hashes_resolves_to_confirmed_fills() {
    let venue = mock_venue().await;
    let client = authed_client(&venue);
    // The venue knows the order (matched) and one CONFIRMED trade
    // with a published settlement hash.
    venue.set_order("0xord1", "matched", 25.0, 25.0);
    venue.set_trades(vec![json!({
        "id": "trade-1",
        "taker_order_id": "0xord1",
        "market": CONDITION,
        "asset_id": YES,
        "side": "BUY",
        "size": "25",
        "price": "0.4",
        "status": "CONFIRMED",
        "match_time": "1713398400",
        "transaction_hash": "0xsettle1",
        "bucket_index": 0
    })]);

    let a = acceptance("matched", "0xord1", &["trade-1"], &[]);
    assert_eq!(a.commit_state(), CommitState::MatchedAwaitingSettlement);

    let verdict = poll_once(&client, &pending_of(&a)).await.expect("poll");
    match verdict {
        BackfillVerdict::Settled { fills, failed } => {
            assert!(failed.is_empty(), "{failed:?}");
            assert_eq!(fills.len(), 1);
            assert_eq!(fills[0].trade_id, "trade-1");
            assert_eq!(fills[0].transaction_hash, "0xsettle1");
            assert_eq!(fills[0].size, 25.0);
            assert_eq!(fills[0].price, 0.4);
        }
        other => panic!("expected Settled, got {other:?}"),
    }
}

#[tokio::test]
async fn delayed_acceptances_stay_pending_and_book_nothing() {
    let venue = mock_venue().await;
    let client = authed_client(&venue);
    // The venue holds the order in `delayed` with no trades.
    venue.set_order("0xdelayed", "delayed", 0.0, 25.0);

    let a = acceptance("delayed", "0xdelayed", &[], &[]);
    assert_eq!(a.commit_state(), CommitState::PendingMatch);
    assert!(!a.reports_fills());

    let verdict = poll_once(&client, &pending_of(&a)).await.expect("poll");
    match verdict {
        BackfillVerdict::StillPending { trades } => {
            // No trades exist yet — nothing to book, nothing invented.
            assert!(trades.is_empty(), "{trades:?}");
        }
        other => panic!("expected StillPending, got {other:?}"),
    }
}

#[tokio::test]
async fn an_order_the_venue_forgot_is_dropped() {
    let venue = mock_venue().await;
    let client = authed_client(&venue);
    // No order scripted → /data/order answers 404.
    let a = acceptance("delayed", "0xghost", &[], &[]);
    let verdict = poll_once(&client, &pending_of(&a)).await.expect("poll");
    assert_eq!(verdict, BackfillVerdict::OrderGoneOnVenue);

    // A terminal-cancelled venue order closes the question the same
    // way: no trades can ever appear for it.
    venue.set_order("0xcancelled", "cancelled", 0.0, 25.0);
    let a = acceptance("matched", "0xcancelled", &["t-9"], &[]);
    let verdict = poll_once(&client, &pending_of(&a)).await.expect("poll");
    assert_eq!(verdict, BackfillVerdict::OrderGoneOnVenue);
}

#[tokio::test]
async fn a_failed_trade_is_reported_never_booked_as_a_fill() {
    let venue = mock_venue().await;
    let client = authed_client(&venue);
    venue.set_order("0xord2", "matched", 10.0, 10.0);
    venue.set_trades(vec![json!({
        "id": "trade-bad",
        "taker_order_id": "0xord2",
        "market": CONDITION,
        "asset_id": YES,
        "side": "BUY",
        "size": "10",
        "price": "0.4",
        "status": "FAILED",
        "match_time": "1713398400",
        "transaction_hash": "",
        "bucket_index": 0
    })]);
    let a = acceptance("matched", "0xord2", &["trade-bad"], &[]);
    let verdict = poll_once(&client, &pending_of(&a)).await.expect("poll");
    match verdict {
        BackfillVerdict::Settled { fills, failed } => {
            assert!(fills.is_empty(), "{fills:?}");
            assert_eq!(
                failed,
                vec![("trade-bad".to_string(), "FAILED".to_string())]
            );
        }
        other => panic!("expected Settled with a failure, got {other:?}"),
    }
}

#[tokio::test]
async fn a_settlement_wait_times_out_as_still_settling_not_failure() {
    let venue = mock_venue().await;
    let client = authed_client(&venue);
    // The trade stays MINED (no finality) for the whole wait.
    venue.set_order("0xord3", "matched", 5.0, 5.0);
    venue.set_trades(vec![json!({
        "id": "trade-slow",
        "taker_order_id": "0xord3",
        "market": CONDITION,
        "asset_id": YES,
        "side": "BUY",
        "size": "5",
        "price": "0.4",
        "status": "MINED",
        "match_time": "1713398400",
        "transaction_hash": "",
        "bucket_index": 0
    })]);
    let a = acceptance("matched", "0xord3", &["trade-slow"], &[]);
    let outcome = wait_for_settlement(
        &client,
        "0xord3",
        &a.trade_ids,
        Duration::from_millis(300),
        Duration::from_millis(80),
    )
    .await
    .expect("wait runs");
    match outcome {
        module_polymarket::trade_resolution::SettlementOutcome::StillSettling { trades } => {
            assert_eq!(trades.len(), 1);
            assert_eq!(trades[0].settlement.as_str(), "settling");
        }
        other => panic!("expected StillSettling, got {other:?}"),
    }
}

#[tokio::test]
async fn a_live_pipeline_submission_registers_its_own_async_acceptance() {
    let venue = mock_venue().await;
    venue.set_balance(100_000_000, 100_000_000);
    // The venue answers matched WITH trade ids but WITHOUT hashes —
    // exactly the async gap.
    venue.set_post(PostBehaviour::Raw(json!({
        "success": true,
        "orderID": "",
        "status": "matched",
        "takingAmount": "25000000",
        "makingAmount": "10000000",
        "tradeIDs": ["trade-p1"],
        "transactionsHashes": [],
        "errorMsg": ""
    })));
    let (state, _oms) = state_with_oms(live_config(&venue));
    let bot = live_bot(&state, Arc::new(MemoryPolyStore::new())).await;
    let pcfg = poly_cfg(&state).await;

    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Live),
            &market(),
            &quotes(),
            &pcfg,
        )
        .await;
    assert_eq!(out.stage, PolyStage::Filled, "{out:?}");

    // The pipeline registered the acceptance for backfill.
    let pending = bot.async_pending().pending();
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(
        pending[0].commit_state,
        CommitState::MatchedAwaitingSettlement
    );
    assert_eq!(pending[0].trade_ids, vec!["trade-p1".to_string()]);
    assert!(pending[0].transactions_hashes.is_empty());

    // Now the venue publishes the settlement hash; one poll resolves
    // it and the registry is cleared.
    let venue_order_id = pending[0].order_id.clone();
    venue.set_order(&venue_order_id, "matched", 25.0, 25.0);
    venue.set_trades(vec![json!({
        "id": "trade-p1",
        "taker_order_id": venue_order_id,
        "market": CONDITION,
        "asset_id": YES,
        "side": "BUY",
        "size": "25",
        "price": "0.4",
        "status": "CONFIRMED",
        "match_time": "1713398400",
        "transaction_hash": "0xsettled-p1",
        "bucket_index": 0
    })]);
    let client = authed_client(&venue);
    let verdict = poll_once(&client, &pending[0]).await.expect("poll");
    match verdict {
        BackfillVerdict::Settled { fills, failed } => {
            assert!(failed.is_empty());
            assert_eq!(fills.len(), 1);
            assert_eq!(fills[0].transaction_hash, "0xsettled-p1");
        }
        other => panic!("expected Settled, got {other:?}"),
    }
}

#[tokio::test]
async fn the_registry_only_holds_acceptances_that_owe_facts() {
    let reg = AsyncPendingRegistry::new();
    let now = chrono::Utc::now();
    let stale = chrono::Duration::seconds(900);
    // settled (matched + hashes): not recorded.
    assert!(!reg
        .record(&acceptance("matched", "0xa", &["t"], &["0xh"]), now, stale)
        .unwrap());
    // live: not recorded.
    assert!(!reg
        .record(&acceptance("live", "0xb", &[], &[]), now, stale)
        .unwrap());
    // matched-without-hashes and delayed: recorded.
    assert!(reg
        .record(&acceptance("matched", "0xc", &["t"], &[]), now, stale)
        .unwrap());
    assert!(reg
        .record(&acceptance("delayed", "0xd", &[], &[]), now, stale)
        .unwrap());
    assert_eq!(reg.len(), 2);
}
