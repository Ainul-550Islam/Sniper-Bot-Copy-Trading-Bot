//! Durable PRE-BROADCAST reservation for Polymarket LIVE entries (0079).
//!
//! Polymarket was the last money-moving module without a durable claim in
//! front of its venue POST: the CLOB `POST /order` ran behind an in-process
//! ownership lease only, so a restarted process or a second replica whose
//! lease view had not converged could POST the same venue entry twice while
//! the first attempt's outcome was still unknown.
//!
//! These tests pin the two properties that matter:
//!   * a sink that REFUSES the claim (another owner holds it, or the durable
//!     store is unreachable) means **no POST reaches the venue**, the OMS order
//!     is failed and the tracked order is dropped;
//!   * a sink that GRANTS the claim lets the pipeline proceed exactly as
//!     before, and the claim it was asked for is the venue identity
//!     `poly:entry:{token_id}`.

mod common;

use std::sync::Arc;
use std::sync::Mutex;

use bot_core::db::repo::IntentRecord;
use bot_core::models::ExecutionMode;
use bot_core::recovery::IntentSink;
use common::*;
use module_polymarket::orders::{PolyStage, RejectReason};
use module_polymarket::store::MemoryPolyStore;

/// Records every reservation it is asked for and answers `granted`.
struct RecordingSink {
    granted: bool,
    keys: Mutex<Vec<String>>,
    records: Mutex<Vec<IntentRecord>>,
}

impl RecordingSink {
    fn new(granted: bool) -> Arc<Self> {
        Arc::new(RecordingSink {
            granted,
            keys: Mutex::new(Vec::new()),
            records: Mutex::new(Vec::new()),
        })
    }

    fn keys(&self) -> Vec<String> {
        self.keys.lock().unwrap().clone()
    }

    fn records(&self) -> Vec<IntentRecord> {
        self.records.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl IntentSink for RecordingSink {
    async fn record(&self, rec: IntentRecord) {
        self.records.lock().unwrap().push(rec);
    }

    async fn record_checked(&self, rec: IntentRecord) -> bot_core::BotResult<()> {
        self.records.lock().unwrap().push(rec);
        Ok(())
    }

    async fn reserve(&self, reservation_key: &str, rec: &IntentRecord) -> bot_core::BotResult<()> {
        self.keys.lock().unwrap().push(reservation_key.to_string());
        self.records.lock().unwrap().push(rec.clone());
        if self.granted {
            Ok(())
        } else {
            Err(bot_core::BotError::invalid(
                "trade poly:entry is reserved by another replica (epoch 2)",
            ))
        }
    }

    async fn link(&self, _intent_id: &str, _signature: &str) {}

    async fn abandon(&self, _intent_id: &str) {}
}

#[tokio::test]
async fn a_refused_reservation_stops_the_live_clob_post() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(live_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let sink = RecordingSink::new(false);
    let bot = live_bot(&state, store.clone())
        .await
        .with_intent_sink(sink.clone());
    let cfg = poly_cfg(&state).await;

    let sig = signal(decision(), ExecutionMode::Live);
    let out = bot.process_signal(&sig, &market(), &quotes(), &cfg).await;

    assert_eq!(
        out.stage,
        PolyStage::Failed,
        "a refused durable claim must fail the signal: {out:?}"
    );
    assert_eq!(out.reject_reason, Some(RejectReason::OmsRejected));
    assert_eq!(
        venue.posted_orders(),
        0,
        "no CLOB order may reach the venue when the durable claim is refused"
    );

    // The claim the pipeline asked for is the venue identity, and it carries
    // the frozen order economics.
    let keys = sink.keys();
    assert_eq!(keys, vec![format!("poly:entry:{}", decision().token_id)]);
    let records = sink.records();
    let rec = records.last().expect("a reservation record");
    assert_eq!(rec.module, "polymarket");
    assert_eq!(rec.side.to_lowercase(), "buy");
    assert!(rec.intent_id.starts_with("poly-"), "{}", rec.intent_id);

    // The OMS order is failed, and nothing is left tracked for the venue.
    let oms_order = oms
        .list(10)
        .await
        .into_iter()
        .find(|o| o.idempotency_key == sig.intent_key())
        .expect("the intent still owns exactly one OMS order");
    assert_ne!(oms_order.status, bot_core::oms::OrderStatus::Filled);
    assert!(store.orders().await.is_empty(), "no venue order journaled");
}

#[tokio::test]
async fn a_granted_reservation_lets_the_live_post_run_and_is_keyed_by_venue_identity() {
    let venue = mock_venue().await;
    let (state, _oms) = state_with_oms(live_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let sink = RecordingSink::new(true);
    let bot = live_bot(&state, store.clone())
        .await
        .with_intent_sink(sink.clone());
    let cfg = poly_cfg(&state).await;

    let sig = signal(decision(), ExecutionMode::Live);
    let out = bot.process_signal(&sig, &market(), &quotes(), &cfg).await;

    assert!(
        out.reached_venue(),
        "a granted claim must not block the pipeline: {out:?}"
    );
    assert_eq!(
        venue.posted_orders(),
        1,
        "exactly one venue POST for one granted claim"
    );
    assert_eq!(
        sink.keys(),
        vec![format!("poly:entry:{}", decision().token_id)],
        "the durable identity must be the venue entry, not the attempt"
    );
    assert_eq!(
        store.orders().await.len(),
        1,
        "the venue claim is journaled"
    );
}

#[tokio::test]
async fn paper_entries_never_consult_the_reservation_and_never_post() {
    let venue = mock_venue().await;
    let (state, _oms) = state_with_oms(base_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let sink = RecordingSink::new(false);
    let bot = paper_bot(&state, store)
        .await
        .with_intent_sink(sink.clone());
    let cfg = poly_cfg(&state).await;

    let sig = signal(decision(), ExecutionMode::Paper);
    let out = bot.process_signal(&sig, &market(), &quotes(), &cfg).await;

    assert_eq!(out.stage, PolyStage::Filled, "{out:?}");
    assert!(sink.keys().is_empty(), "paper has nothing to reserve");
    assert_eq!(venue.posted_orders(), 0);
}
