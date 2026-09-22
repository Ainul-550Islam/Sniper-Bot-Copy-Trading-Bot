//! Dedup and ordering through the real pipeline (TASK 3 test 14): one
//! authoritative dedup keyed by the event (not the feed's signature mark),
//! cross-source duplicates, distinct events sharing a signature, strict vs
//! advisory ordering, per-leader cursors and sequence gaps.

mod common;

use common::*;

use bot_core::models::{BotModule, PositionSide};
use bot_core::obs::metrics;
use module_copy::event::{CopyStage, EventSource, LeaderTradeEvent, RejectReason};
use module_copy::event_dedup;
use module_copy::event_ordering::OrderingVerdict;

#[tokio::test]
async fn the_same_event_from_two_feeds_is_mirrored_exactly_once() {
    let mut w = CopyWorld::new(copy_config()).await;
    let raw = leader_buy(LEADER, w.mint, 1, 0.5);
    let via_ws = event(&raw, EventSource::PumpPortal, 1);
    let mut via_poll = event(&raw, EventSource::LogsPoll, 7);
    via_poll.observed_at = raw.observed_at + chrono::Duration::milliseconds(800);
    assert_eq!(via_ws.event_id, via_poll.event_id);
    assert_eq!(via_ws.dedup_key(), via_poll.dedup_key());

    let first = w.process(&via_ws).await;
    assert_eq!(first.stage, CopyStage::Filled, "{:?}", first.rejection);
    let second = w.process(&via_poll).await;
    let r = second.rejection.expect("duplicate");
    assert_eq!(r.reason, RejectReason::DuplicateEvent);
    assert_eq!(r.stage, CopyStage::LeaderResolved);
    assert_eq!(w.state.open_positions_for(BotModule::Copy).await.len(), 1);
    // TASK 5: the one mirrored fill reached the global ledger once, tagged
    // with the leader-scoped strategy label and the copy intent.
    let ledger = w.state.ledger();
    assert_eq!(ledger.len().await, 1);
    let ev = &ledger.events().await[0].event;
    assert_eq!(ev.module, BotModule::Copy);
    assert!(ev.strategy.starts_with("copy:"), "{}", ev.strategy);
    assert!(ev.correlation_id.is_some());
    let position = &w.state.open_positions_for(BotModule::Copy).await[0];
    assert_eq!(ev.position_id.as_deref(), Some(position.id.as_str()));
    assert!((ledger.open_positions().await[0].qty - position.qty).abs() < 1e-9);
    // A third delivery from the "same" feed is equally refused.
    assert_eq!(
        w.process(&via_ws).await.rejection.unwrap().reason,
        RejectReason::DuplicateEvent
    );
    // Duplicates are not journaled twice and do not count as leader rejections.
    let reg = w.bot.leaders();
    let reg = reg.read().await;
    let l = reg.get(LEADER).unwrap();
    assert_eq!(l.stats.events_seen, 3);
    assert_eq!(l.stats.rejected, 0);
    assert_eq!(l.stats.mirrored, 1);
    assert!(
        metrics::global()
            .counter("copy_dedup_total", "", &[("outcome", "duplicate")])
            .get()
            >= 2
    );
}

#[tokio::test]
async fn feed_signature_marks_never_starve_the_pipeline() {
    // Regression for the pre-TASK-3 bug: the feeds mark every signature they
    // decode (fetch suppression) and the pipeline used to re-mark the SAME
    // key, dropping every polled / Geyser event as a duplicate.
    let mut w = CopyWorld::new(copy_config()).await;
    let raw = leader_buy(LEADER, w.mint, 2, 0.5);
    assert!(
        w.state.mark_signature_seen(&raw.signature).await,
        "feed marks first"
    );
    let e = event(&raw, EventSource::TransactionSubscribe, 1);
    let out = w.process(&e).await;
    assert_eq!(out.stage, CopyStage::Filled, "{:?}", out.rejection);
    // And the pipeline did not consume the feed namespace either.
    assert!(!w.state.mark_signature_seen(&raw.signature).await);
    assert!(w.state.mark_signature_seen("some-other-signature").await);
}

#[tokio::test]
async fn one_signature_two_mints_or_two_sides_are_distinct_events() {
    let mut w = CopyWorld::new(copy_config()).await;
    let other = w.new_token();
    let mut raw_a = leader_buy(LEADER, w.mint, 3, 0.5);
    let mut raw_b = leader_buy(LEADER, other, 3, 0.5);
    raw_b.signature = raw_a.signature.clone();
    raw_a.slot = 600;
    raw_b.slot = 600;
    let a = event(&raw_a, EventSource::PumpPortal, 1);
    let b = event(&raw_b, EventSource::PumpPortal, 2);
    assert_ne!(a.event_id, b.event_id);
    assert_eq!(w.process(&a).await.stage, CopyStage::Filled);
    assert_eq!(w.process(&b).await.stage, CopyStage::Filled);
    assert_eq!(w.state.open_positions_for(BotModule::Copy).await.len(), 2);

    // Same signature, opposite side → a distinct event (mirrored exit).
    let mut raw_sell = raw_a.clone();
    raw_sell.side = PositionSide::Short;
    let s = event(&raw_sell, EventSource::PumpPortal, 3);
    assert_ne!(s.event_id, a.event_id);
    let out = w.process(&s).await;
    assert_eq!(out.stage, CopyStage::ExitMirrored, "{:?}", out.rejection);
    assert_eq!(w.state.open_positions_for(BotModule::Copy).await.len(), 1);
    assert_eq!(w.state.seen_copy_event_count().await, 3);
}

#[tokio::test]
async fn advisory_ordering_processes_late_events_and_counts_them() {
    let mut w = CopyWorld::new(copy_config()).await;
    let m1 = w.new_token();
    let m2 = w.new_token();
    let mut newer = w.buy_in(m1, 4);
    newer.slot = 1_000;
    let mut older = w.buy_in(m2, 5);
    older.slot = 900;
    assert_eq!(w.process(&newer).await.stage, CopyStage::Filled);
    let out = w.process(&older).await;
    assert_eq!(
        out.stage,
        CopyStage::Filled,
        "advisory mode mirrors late events"
    );
    let c = w.bot.ordering().cursor(LEADER).expect("cursor");
    assert_eq!(c.last_slot, 1_000);
    assert_eq!(c.last_signature, newer.signature);
    assert_eq!(c.out_of_order, 1);
    assert_eq!(c.observed, 2);
    assert!(
        metrics::global()
            .counter("copy_ordering_total", "", &[("verdict", "out_of_order")])
            .get()
            >= 1
    );
}

#[tokio::test]
async fn strict_ordering_rejects_late_events_but_not_same_or_unknown_slots() {
    let mut cfg = copy_config();
    cfg.copy.strict_ordering = true;
    let mut w = CopyWorld::new(cfg).await;
    let m1 = w.new_token();
    let m2 = w.new_token();
    let m3 = w.new_token();
    let m4 = w.new_token();
    let mut first = w.buy_in(m1, 6);
    first.slot = 1_000;
    assert_eq!(w.process(&first).await.stage, CopyStage::Filled);

    let mut late = w.buy_in(m2, 7);
    late.slot = 999;
    let out = w.process(&late).await;
    let r = out.rejection.expect("rejected");
    assert_eq!(r.reason, RejectReason::OutOfOrder);
    assert_eq!(r.stage, CopyStage::Deduplicated);
    assert!(r.detail.contains("999"), "{}", r.detail);
    // The late event was still claimed by dedup: a redelivery is a duplicate,
    // not a second OUT_OF_ORDER decision.
    assert_eq!(
        w.process(&late).await.rejection.unwrap().reason,
        RejectReason::DuplicateEvent
    );

    let mut same_slot = w.buy_in(m3, 8);
    same_slot.slot = 1_000;
    assert_eq!(w.process(&same_slot).await.stage, CopyStage::Filled);
    let mut unknown = w.buy_in(m4, 9);
    unknown.slot = 0;
    assert_eq!(w.process(&unknown).await.stage, CopyStage::Filled);
    assert_eq!(w.bot.ordering().cursor(LEADER).unwrap().last_slot, 1_000);

    // Hot reload to advisory: the next late event mirrors.
    let mut relaxed = w.cfg.clone();
    relaxed.copy.strict_ordering = false;
    w.reload(relaxed).await;
    assert!(!w.bot.ordering().is_strict());
    let m5 = w.new_token();
    let mut late2 = w.buy_in(m5, 10);
    late2.slot = 5;
    assert_eq!(w.process(&late2).await.stage, CopyStage::Filled);
}

#[tokio::test]
async fn cursors_are_per_leader_and_gaps_are_reported_not_rejected() {
    let mut cfg = copy_config();
    cfg.copy.strict_ordering = true;
    cfg.copy.wallets.push(rule(LEADER_B));
    let mut w = CopyWorld::new(cfg).await;
    let m1 = w.new_token();
    let m2 = w.new_token();
    let m3 = w.new_token();
    let mut a = w.buy_in(m1, 11);
    a.slot = 5_000;
    assert_eq!(w.process(&a).await.stage, CopyStage::Filled);
    // Leader B's first event sits far below A's cursor: independent cursors.
    let mut b = event(
        &leader_buy(LEADER_B, m2, 12, 0.5),
        EventSource::PumpPortal,
        1,
    );
    b.slot = 10;
    assert_eq!(
        w.process(&b).await.stage,
        CopyStage::Filled,
        "B has its own cursor"
    );
    assert_eq!(w.bot.ordering().cursor(LEADER_B).unwrap().last_slot, 10);
    assert_eq!(w.bot.ordering().cursor(LEADER).unwrap().last_slot, 5_000);

    // A sequence gap on A's source is surfaced but the event is processed.
    let before = metrics::global()
        .counter("copy_ordering_total", "", &[("verdict", "gap")])
        .get();
    let mut gap = w.buy_in(m3, 13);
    gap.slot = 5_001;
    gap.source_sequence = 40; // previous A sequence from this source was 11
    let out = w.process(&gap).await;
    assert_eq!(out.stage, CopyStage::Filled, "{:?}", out.rejection);
    assert_eq!(
        metrics::global()
            .counter("copy_ordering_total", "", &[("verdict", "gap")])
            .get(),
        before + 1
    );
    let c = w.bot.ordering().cursor(LEADER).unwrap();
    assert_eq!(c.gaps, 28);
    assert_eq!(c.last_sequence[&EventSource::PumpPortal], 40);
}

#[tokio::test]
async fn seeding_the_dedup_makes_replayed_events_duplicates() {
    let mut w = CopyWorld::new(copy_config()).await;
    let e = w.buy(14);
    assert_eq!(event_dedup::seed(&w.state, [e.dedup_key()]).await, 1);
    assert_eq!(
        w.process(&e).await.rejection.unwrap().reason,
        RejectReason::DuplicateEvent
    );
    assert!(w.state.open_positions_for(BotModule::Copy).await.is_empty());
    let verdict = OrderingVerdict::OutOfOrder {
        newest_slot: 5,
        behind_by: 2,
    };
    assert_eq!(verdict.as_str(), "out_of_order");
    assert!(verdict.is_out_of_order());
    let fresh: LeaderTradeEvent = w.buy(15);
    assert!(!event_dedup::already_seen(&w.state, &fresh).await);
}
