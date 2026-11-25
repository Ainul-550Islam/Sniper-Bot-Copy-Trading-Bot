//! The venue check is now ACTED ON — and only in the one safe direction.
//!
//! `venue_state` decides what the CLOB holds; this suite pins what the pipeline
//! does with that answer for an order stuck in `Submitted`/`Unknown`:
//!
//! * `landed` → the durable claim is KEPT (the normal settle path owns it);
//! * `absent` + never acknowledged → the claim is RELEASED as an established
//!   non-send and the order closes as `failed`, so the token is tradeable again;
//! * `absent` but once acknowledged → nothing is released (a purged full match
//!   looks identical to a 404);
//! * `mismatch` / `unknown` → nothing is released, held for a human / retried;
//! * a check about a different order id is refused outright.
//!
//! The sink here records the release calls, so the assertions are about the
//! DURABLE effect, not just the local state machine.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use bot_core::db::repo::IntentRecord;
use bot_core::recovery::IntentSink;
use chrono::Utc;
use module_polymarket::clob::ClobOrder;
use module_polymarket::orders::{LocalOrderState, TrackedOrder};
use module_polymarket::store::MemoryPolyStore;
use module_polymarket::venue_settle::VenueSettlement;
use module_polymarket::venue_state::{judge, VenuePresence, VenueStateCheck};
use module_polymarket::PolyBot;

const TOKEN: &str = "71321045679252212594626385532706912750332728571942532289631379312455583992563";
const DERIVED: &str = "0xabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd";

/// Records every release and answers with the configured verdict.
struct RecordingSink {
    releases: AtomicUsize,
    established_non_send: AtomicUsize,
    answer: Option<bool>,
}

#[async_trait::async_trait]
impl IntentSink for RecordingSink {
    async fn record(&self, _rec: IntentRecord) {}
    async fn link(&self, _i: &str, _s: &str) {}
    async fn abandon(&self, _i: &str) {}
    async fn release_reservation(&self, _key: &str, established_non_send: bool) -> Option<bool> {
        self.releases.fetch_add(1, Ordering::SeqCst);
        if established_non_send {
            self.established_non_send.fetch_add(1, Ordering::SeqCst);
        }
        self.answer
    }
}

fn sink(answer: Option<bool>) -> Arc<RecordingSink> {
    Arc::new(RecordingSink {
        releases: AtomicUsize::new(0),
        established_non_send: AtomicUsize::new(0),
        answer,
    })
}

/// An order the venue has NOT acknowledged (no venue status).
fn unacknowledged_order() -> TrackedOrder {
    TrackedOrder {
        venue_order_id: DERIVED.to_string(),
        order_id: "oms-1".to_string(),
        signal_id: "psig_1".to_string(),
        condition_id: common::CONDITION.to_string(),
        token_id: TOKEN.to_string(),
        outcome: "Yes".to_string(),
        question: "q".to_string(),
        is_buy: true,
        neg_risk: false,
        order_type: "GTC".to_string(),
        limit_price: 0.42,
        size_tokens: 10.0,
        size_matched: 0.0,
        mode: bot_core::models::ExecutionMode::Live,
        state: LocalOrderState::Submitted,
        venue_status: String::new(),
        expiration: 0,
        position_id: None,
        signature: None,
        submitted_at: Utc::now(),
        updated_at: Utc::now(),
        booked_trade_ids: Vec::new(),
        trade_matched: 0.0,
    }
}

fn acknowledged_order() -> TrackedOrder {
    let mut t = unacknowledged_order();
    t.venue_status = "live".into();
    t
}

fn check(presence: VenuePresence) -> VenueStateCheck {
    match presence {
        VenuePresence::Landed => judge(
            DERIVED,
            TOKEN,
            Some(&ClobOrder {
                id: DERIVED.to_string(),
                status: "matched".into(),
                asset_id: TOKEN.into(),
                size_matched: "10".into(),
                ..Default::default()
            }),
            &[],
            None,
        ),
        VenuePresence::Absent => judge(DERIVED, TOKEN, None, &[], None),
        VenuePresence::Mismatch => judge(
            DERIVED,
            TOKEN,
            None,
            &[ClobOrder {
                id: "0xfeed".into(),
                status: "live".into(),
                asset_id: TOKEN.into(),
                ..Default::default()
            }],
            None,
        ),
        VenuePresence::Unknown => judge(
            DERIVED,
            TOKEN,
            None,
            &[],
            Some(&module_polymarket::error::PolyError::http("reset")),
        ),
    }
}

async fn bot_with(sink: Arc<RecordingSink>) -> PolyBot {
    let venue = common::mock_venue().await;
    let state = common::state_with(common::live_config(&venue));
    PolyBot::new(state)
        .await
        .expect("bot")
        .with_store(Arc::new(MemoryPolyStore::default()))
        .with_intent_sink(sink)
}

#[tokio::test]
async fn a_landing_is_never_released() {
    let s = sink(Some(true));
    let bot = bot_with(s.clone()).await;
    let mut t = unacknowledged_order();
    let out: VenueSettlement = bot
        .settle_claim_with_venue(&mut t, &check(VenuePresence::Landed))
        .await
        .expect("settle");
    assert_eq!(out.presence, VenuePresence::Landed);
    assert!(!out.claim_released);
    assert!(!out.order_closed);
    assert_eq!(
        t.state,
        LocalOrderState::Submitted,
        "a landing changes nothing locally"
    );
    assert_eq!(
        s.releases.load(Ordering::SeqCst),
        0,
        "the sink must not be asked"
    );
}

#[tokio::test]
async fn a_never_acknowledged_absence_releases_the_claim_and_closes_the_order() {
    let s = sink(Some(true));
    let bot = bot_with(s.clone()).await;
    let mut t = unacknowledged_order();
    let out = bot
        .settle_claim_with_venue(&mut t, &check(VenuePresence::Absent))
        .await
        .expect("settle");
    assert_eq!(out.presence, VenuePresence::Absent);
    assert!(out.claim_released, "the token must become tradeable again");
    assert!(out.order_closed);
    assert_eq!(t.state, LocalOrderState::Failed);
    assert_eq!(s.releases.load(Ordering::SeqCst), 1);
    assert_eq!(
        s.established_non_send.load(Ordering::SeqCst),
        1,
        "absence is only ever released as an established non-send"
    );
    assert_eq!(out.action(), "claim_released_absent");
}

#[tokio::test]
async fn a_once_acknowledged_order_that_vanished_is_held() {
    let s = sink(Some(true));
    let bot = bot_with(s.clone()).await;
    let mut t = acknowledged_order();
    let out = bot
        .settle_claim_with_venue(&mut t, &check(VenuePresence::Absent))
        .await
        .expect("settle");
    assert!(
        !out.claim_released,
        "a purged full match looks exactly like a 404"
    );
    assert!(!out.order_closed);
    assert_eq!(t.state, LocalOrderState::Submitted);
    assert_eq!(s.releases.load(Ordering::SeqCst), 0);
    assert!(out.detail.contains("once acknowledged"), "{}", out.detail);
}

#[tokio::test]
async fn a_sink_that_refuses_to_release_keeps_the_claim_blocked() {
    let s = sink(Some(false));
    let bot = bot_with(s.clone()).await;
    let mut t = unacknowledged_order();
    let out = bot
        .settle_claim_with_venue(&mut t, &check(VenuePresence::Absent))
        .await
        .expect("settle");
    assert!(!out.claim_released, "the durable store has the last word");
    assert!(out.detail.contains("kept"), "{}", out.detail);
    // The local order still closes: the venue's answer is the evidence.
    assert_eq!(t.state, LocalOrderState::Failed);
}

#[tokio::test]
async fn a_sink_without_claim_tracking_reports_no_release() {
    let s = sink(None);
    let bot = bot_with(s.clone()).await;
    let mut t = unacknowledged_order();
    let out = bot
        .settle_claim_with_venue(&mut t, &check(VenuePresence::Absent))
        .await
        .expect("settle");
    assert!(!out.claim_released);
    assert_eq!(s.releases.load(Ordering::SeqCst), 1, "it was still asked");
}

#[tokio::test]
async fn mismatch_and_unknown_release_nothing() {
    for presence in [VenuePresence::Mismatch, VenuePresence::Unknown] {
        let s = sink(Some(true));
        let bot = bot_with(s.clone()).await;
        let mut t = unacknowledged_order();
        let out = bot
            .settle_claim_with_venue(&mut t, &check(presence))
            .await
            .expect("settle");
        assert!(!out.claim_released, "{presence:?} must not release");
        assert_eq!(t.state, LocalOrderState::Submitted, "{presence:?}");
        assert_eq!(s.releases.load(Ordering::SeqCst), 0, "{presence:?}");
    }
}

#[tokio::test]
async fn a_settled_order_is_not_settled_twice() {
    let s = sink(Some(true));
    let bot = bot_with(s.clone()).await;
    let mut t = unacknowledged_order();
    let first = bot
        .settle_claim_with_venue(&mut t, &check(VenuePresence::Absent))
        .await
        .expect("settle");
    assert!(first.claim_released);
    let second = bot
        .settle_claim_with_venue(&mut t, &check(VenuePresence::Absent))
        .await
        .expect("settle");
    assert!(
        !second.claim_released,
        "an already-failed order is informational only"
    );
    assert_eq!(s.releases.load(Ordering::SeqCst), 1, "asked exactly once");
}

#[tokio::test]
async fn a_check_for_another_order_is_refused() {
    let s = sink(Some(true));
    let bot = bot_with(s.clone()).await;
    let mut t = unacknowledged_order();
    let other = judge(
        "0x9999999999999999999999999999999999999999999999999999999999999999",
        TOKEN,
        None,
        &[],
        None,
    );
    assert!(bot.settle_claim_with_venue(&mut t, &other).await.is_err());
    assert_eq!(s.releases.load(Ordering::SeqCst), 0);
    assert_eq!(t.state, LocalOrderState::Submitted);
}

#[tokio::test]
async fn a_paper_bot_without_a_venue_check_is_unaffected() {
    // Sanity: the settlement path never runs unless a check is supplied, so a
    // bot with no sink keeps its old behaviour.
    let venue = common::mock_venue().await;
    let state = common::state_with(common::live_config(&venue));
    let bot = PolyBot::new(state).await.expect("bot");
    let mut t = unacknowledged_order();
    let out = bot
        .settle_claim_with_venue(&mut t, &check(VenuePresence::Absent))
        .await
        .expect("settle");
    assert!(!out.claim_released);
    assert_eq!(t.state, LocalOrderState::Failed);
    let _ = Utc::now();
}
