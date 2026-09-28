//! Strategy verdicts → frozen signals → sized orders (TASK 4). Covers the
//! hardened `value`/`search` strategies end to end (both basket legs are
//! booked, sizes are rounded down to the venue's 0.01 grid, decisions are
//! deterministic so a re-scan is a duplicate), GTD/FOK order types reaching
//! the venue as configured, and the Polymarket exposure caps that live in
//! the shared `RiskEngine` (resting-order cap, total/per-market exposure,
//! concurrent positions, price band).

mod common;

use std::sync::Arc;

use bot_core::models::{BotModule, ExecutionMode};
use chrono::Utc;
use common::*;
use module_polymarket::orders::{LocalOrderState, PolyStage, RejectReason};
use module_polymarket::store::MemoryPolyStore;
use module_polymarket::strategy::{evaluate_market, round_size, Verdict};

#[tokio::test]
async fn value_basket_verdicts_flow_into_two_booked_legs() {
    let venue = mock_venue().await;
    let (state, oms) = state_with_oms(base_config(&venue));
    let store = Arc::new(MemoryPolyStore::new());
    let bot = paper_bot(&state, store.clone()).await;
    let cfg = poly_cfg(&state).await;

    // asks 0.40 + 0.55 = 0.95 → edge 0.05 ≥ min_edge 0.03; 10 USDC buys
    // floor(10 / 0.95, 0.01) = 10.52 baskets.
    let verdicts = evaluate_market(&market(), &quotes(), &cfg, Utc::now());
    let decisions: Vec<_> = verdicts
        .iter()
        .filter_map(|v| match v {
            Verdict::Enter(d) => Some(d.clone()),
            Verdict::Skip { .. } => None,
        })
        .collect();
    assert_eq!(decisions.len(), 2, "{verdicts:?}");
    assert_eq!(decisions[0].token_id, YES);
    assert_eq!(decisions[1].token_id, NO);
    for d in &decisions {
        assert!((d.size_tokens - 10.52).abs() < 1e-9, "{d:?}");
        assert_eq!(
            d.size_tokens,
            round_size(d.size_tokens),
            "already on the 0.01 grid"
        );
    }
    let total_stake: f64 = decisions.iter().map(|d| d.stake_usd).sum();
    assert!(
        total_stake <= cfg.stake_usd + 1e-9,
        "basket never exceeds stake_usd: {total_stake}"
    );

    let mut first_ids = Vec::new();
    for d in &decisions {
        let sig = bot.build_signal(d.clone(), "value", &market(), &cfg).await;
        assert_eq!(sig.order_type, "GTC");
        assert_eq!(sig.expiration, 0, "GTC carries no expiry");
        assert_eq!(sig.mode, ExecutionMode::Paper);
        first_ids.push(sig.signal_id.clone());
        let out = bot.process_signal(&sig, &market(), &quotes(), &cfg).await;
        assert_eq!(out.stage, PolyStage::Filled, "{out:?}");
        assert_eq!(out.size_tokens, Some(10.52));
    }
    let yes = state.find_open(BotModule::Polymarket, YES).await.unwrap();
    let no = state.find_open(BotModule::Polymarket, NO).await.unwrap();
    assert!((yes.qty - 10.52).abs() < 1e-9);
    assert!((no.qty - 10.52).abs() < 1e-9);
    assert!((yes.cost_basis - 10.52 * 0.40).abs() < 1e-6, "{yes:?}");
    assert!((no.cost_basis - 10.52 * 0.55).abs() < 1e-6, "{no:?}");
    assert_eq!(oms.list(10).await.len(), 2);
    assert_eq!(store.fills().await.len(), 2);

    // A re-scan of the same book yields the same intent identities, and
    // the pipeline treats them as duplicates / already in market.
    let again = evaluate_market(&market(), &quotes(), &cfg, Utc::now());
    let mut second_ids = Vec::new();
    for v in &again {
        if let Verdict::Enter(d) = v {
            let sig = bot.build_signal(d.clone(), "value", &market(), &cfg).await;
            second_ids.push(sig.signal_id.clone());
            let out = bot.process_signal(&sig, &market(), &quotes(), &cfg).await;
            assert!(
                matches!(
                    out.reject_reason,
                    Some(RejectReason::DuplicateIntent) | Some(RejectReason::AlreadyInMarket)
                ),
                "{out:?}"
            );
        }
    }
    assert_eq!(
        first_ids, second_ids,
        "signal ids are a pure function of the intent"
    );
    assert_eq!(store.fills().await.len(), 2, "no double booking");
}

#[tokio::test]
async fn search_strategy_sizes_by_stake_and_rounds_down() {
    let venue = mock_venue().await;
    let mut cfg = base_config(&venue);
    cfg.polymarket.strategy = "search".into();
    cfg.polymarket.watch_keywords = vec!["  ".into(), "RESOLVE".into()];
    cfg.polymarket.stake_usd = 7.77;
    let (state, _oms) = state_with_oms(cfg);
    let store = Arc::new(MemoryPolyStore::new());
    let bot = paper_bot(&state, store.clone()).await;
    let cfg = poly_cfg(&state).await;

    let verdicts = evaluate_market(&market(), &quotes(), &cfg, Utc::now());
    assert_eq!(verdicts.len(), 1, "{verdicts:?}");
    let Verdict::Enter(d) = &verdicts[0] else {
        panic!("expected an entry, got {verdicts:?}");
    };
    assert_eq!(d.token_id, YES, "search favours the first outcome");
    // 7.77 / 0.40 = 19.425 → 19.42 on the size grid.
    assert!((d.size_tokens - 19.42).abs() < 1e-9, "{d:?}");
    assert!(d.reason.contains("keyword 'RESOLVE'"), "{}", d.reason);

    let sig = bot.build_signal(d.clone(), "search", &market(), &cfg).await;
    let out = bot.process_signal(&sig, &market(), &quotes(), &cfg).await;
    assert_eq!(out.stage, PolyStage::Filled, "{out:?}");
    assert_eq!(out.size_tokens, Some(19.42));
    assert!(
        (out.approved_stake.unwrap() - 19.42 * 0.40).abs() < 1e-9,
        "{out:?}"
    );
    let pos = state.find_open(BotModule::Polymarket, YES).await.unwrap();
    assert!((pos.qty - 19.42).abs() < 1e-9);

    // Below the venue minimum the strategy itself declines.
    state.update_config(|c| c.polymarket.stake_usd = 1.0).await;
    let cfg = poly_cfg(&state).await;
    let verdicts = evaluate_market(&market(), &quotes(), &cfg, Utc::now());
    assert!(
        matches!(
            &verdicts[0],
            Verdict::Skip { reason, .. } if *reason == module_polymarket::strategy::SkipReason::SizeTooSmall
        ),
        "{verdicts:?}"
    );
}

#[tokio::test]
async fn gtd_expiry_and_order_type_reach_the_venue_as_configured() {
    let venue = mock_venue().await;
    let mut cfg = live_config(&venue);
    cfg.polymarket.order_type = "gtd".into();
    cfg.polymarket.expiration_secs = 600;
    let (state, _oms) = state_with_oms(cfg);
    let store = Arc::new(MemoryPolyStore::new());
    let bot = live_bot(&state, store.clone()).await;
    let cfg = poly_cfg(&state).await;

    let before = Utc::now().timestamp();
    let sig = bot.build_signal(decision(), "value", &market(), &cfg).await;
    assert_eq!(
        sig.order_type, "GTD",
        "order type is normalised to upper case"
    );
    let exp = sig.expiration as i64;
    assert!(
        exp >= before + 600 && exp <= before + 605,
        "expiry {exp} vs now {before}"
    );
    let out = bot.process_signal(&sig, &market(), &quotes(), &cfg).await;
    assert_eq!(out.stage, PolyStage::Resting, "{out:?}");
    let body = venue.last_body("POST", "/clob/order").unwrap();
    assert_eq!(body["orderType"], "GTD");
    assert_eq!(body["owner"], "mock-key");
    assert_eq!(
        body["order"]["timestamp"],
        sig.expiration.to_string(),
        "GTD expiry travels in the V2 timestamp field"
    );
    assert_eq!(body["order"]["side"], "BUY");
    assert_eq!(body["order"]["tokenId"], YES);
    let tracked = bot.tracked_orders().await;
    assert_eq!(tracked[0].expiration, sig.expiration);
    assert_eq!(tracked[0].order_type, "GTD");
    let journaled = store.orders().await;
    assert_eq!(journaled[0].order_type, "GTD");
    assert_eq!(journaled[0].expiration, exp);

    // FOK on the other outcome: the venue answers `matched` and the order
    // is booked in full straight from the submit response.
    state
        .update_config(|c| c.polymarket.order_type = "fok".into())
        .await;
    let cfg = poly_cfg(&state).await;
    venue.set_post(PostBehaviour::Accept {
        status: "matched".into(),
    });
    let sig = bot
        .build_signal(decision_for(NO, "No", 0.55, 20.0), "value", &market(), &cfg)
        .await;
    assert_eq!(sig.order_type, "FOK");
    assert_eq!(sig.expiration, 0, "only GTD carries an expiry");
    let out = bot.process_signal(&sig, &market(), &quotes(), &cfg).await;
    assert_eq!(out.stage, PolyStage::Filled, "{out:?}");
    let body = venue.last_body("POST", "/clob/order").unwrap();
    assert_eq!(body["orderType"], "FOK");
    assert_eq!(body["order"]["timestamp"], "0");
    let t = bot
        .tracked_orders()
        .await
        .into_iter()
        .find(|t| t.token_id == NO)
        .unwrap();
    assert_eq!(t.state, LocalOrderState::Filled);
    assert!((t.size_matched - 20.0).abs() < 1e-9);
    let pos = state.find_open(BotModule::Polymarket, NO).await.unwrap();
    assert!((pos.qty - 20.0).abs() < 1e-9);
    assert_eq!(store.fills().await.len(), 1, "one fill row for the FOK");
}

#[tokio::test]
async fn polymarket_exposure_caps_come_from_the_shared_risk_engine() {
    let venue = mock_venue().await;
    let mut cfg = live_config(&venue);
    cfg.risk.poly_max_open_orders = 1;
    let (state, _oms) = state_with_oms(cfg);
    let store = Arc::new(MemoryPolyStore::new());
    let bot = live_bot(&state, store.clone()).await;
    let cfg = poly_cfg(&state).await;

    // YES rests (10 USDC) …
    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Live),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(out.stage, PolyStage::Resting, "{out:?}");
    let no_leg = || signal(decision_for(NO, "No", 0.55, 20.0), ExecutionMode::Live);

    // … so the resting-order cap refuses the NO leg.
    let out = bot
        .process_signal(&no_leg(), &market(), &quotes(), &cfg)
        .await;
    assert_eq!(
        out.reject_reason,
        Some(RejectReason::OpenOrderCap),
        "{out:?}"
    );
    assert!(
        out.detail.contains("resting polymarket orders"),
        "{}",
        out.detail
    );

    // Total exposure: positions (0) + resting (10) + 11 > 15.
    state
        .update_config(|c| {
            c.risk.poly_max_open_orders = 0;
            c.risk.poly_max_total_exposure_quote = 15.0;
        })
        .await;
    let out = bot
        .process_signal(&no_leg(), &market(), &quotes(), &cfg)
        .await;
    assert_eq!(
        out.reject_reason,
        Some(RejectReason::ExposureCap),
        "{out:?}"
    );
    assert!(out.detail.contains("would exceed cap"), "{}", out.detail);

    // Per-market exposure: resting 10 on this condition + 11 > 12.
    state
        .update_config(|c| {
            c.risk.poly_max_total_exposure_quote = 0.0;
            c.risk.poly_max_market_exposure_quote = 12.0;
        })
        .await;
    let out = bot
        .process_signal(&no_leg(), &market(), &quotes(), &cfg)
        .await;
    assert_eq!(
        out.reject_reason,
        Some(RejectReason::ExposureCap),
        "{out:?}"
    );
    assert!(out.detail.contains("per-market cap"), "{}", out.detail);

    // The NO leg is ONE intent, so the journal keeps ONE row for it that
    // carries the latest verdict.
    let no_rows: Vec<_> = store
        .signals()
        .await
        .into_iter()
        .filter(|s| s.token_id == NO)
        .collect();
    assert_eq!(no_rows.len(), 1, "{no_rows:?}");
    assert_eq!(no_rows[0].reject_reason.as_deref(), Some("EXPOSURE_CAP"));
    assert!(
        no_rows[0].detail.contains("per-market cap"),
        "{}",
        no_rows[0].detail
    );

    // Nothing was posted for the refused attempts; with the caps lifted the
    // same leg goes through.
    assert_eq!(venue.posted_orders(), 1);
    state
        .update_config(|c| c.risk.poly_max_market_exposure_quote = 0.0)
        .await;
    let out = bot
        .process_signal(&no_leg(), &market(), &quotes(), &cfg)
        .await;
    assert_eq!(out.stage, PolyStage::Resting, "{out:?}");
    assert_eq!(venue.posted_orders(), 2);
    let no_row = store
        .signals()
        .await
        .into_iter()
        .find(|s| s.token_id == NO)
        .unwrap();
    assert_eq!(no_row.reject_reason, None, "{no_row:?}");
    assert_eq!(no_row.stage, "RESTING");
}

#[tokio::test]
async fn concurrent_position_cap_and_price_band_are_risk_decisions() {
    let venue = mock_venue().await;
    let mut cfg = base_config(&venue);
    cfg.risk.poly_max_concurrent_positions = 1;
    let (state, _oms) = state_with_oms(cfg);
    let store = Arc::new(MemoryPolyStore::new());
    let bot = paper_bot(&state, store.clone()).await;
    let cfg = poly_cfg(&state).await;

    let out = bot
        .process_signal(
            &signal(decision(), ExecutionMode::Paper),
            &market(),
            &quotes(),
            &cfg,
        )
        .await;
    assert_eq!(out.stage, PolyStage::Filled, "{out:?}");

    // Market B's YES leg is a second concurrent position.
    let verdicts = evaluate_market(&market_b(), &quotes_for(YES_B, NO_B), &cfg, Utc::now());
    let Verdict::Enter(d_b) = &verdicts[0] else {
        panic!("{verdicts:?}");
    };
    assert_eq!(d_b.token_id, YES_B);
    let sig_b = bot
        .build_signal(d_b.clone(), "value", &market_b(), &cfg)
        .await;
    let out = bot
        .process_signal(&sig_b, &market_b(), &quotes_for(YES_B, NO_B), &cfg)
        .await;
    assert_eq!(
        out.reject_reason,
        Some(RejectReason::MaxOpenPositions),
        "{out:?}"
    );
    assert!(state
        .find_open(BotModule::Polymarket, YES_B)
        .await
        .is_none());

    // Lift the cap but move the price floor above the ask: the shared
    // engine's price band refuses the entry (RISK_REJECTED, not a strategy
    // skip — the strategy never sees `risk.*`).
    state
        .update_config(|c| {
            c.risk.poly_max_concurrent_positions = 0;
            c.risk.poly_price_floor = 0.45;
        })
        .await;
    let out = bot
        .process_signal(&sig_b, &market_b(), &quotes_for(YES_B, NO_B), &cfg)
        .await;
    assert_eq!(
        out.reject_reason,
        Some(RejectReason::RiskRejected),
        "{out:?}"
    );
    assert!(out.detail.contains("0.45"), "{}", out.detail);
    assert!(state
        .find_open(BotModule::Polymarket, YES_B)
        .await
        .is_none());

    // Back inside the band the same frozen signal fills.
    state
        .update_config(|c| c.risk.poly_price_floor = 0.02)
        .await;
    let out = bot
        .process_signal(&sig_b, &market_b(), &quotes_for(YES_B, NO_B), &cfg)
        .await;
    assert_eq!(out.stage, PolyStage::Filled, "{out:?}");
    assert_eq!(
        state.open_positions_for(BotModule::Polymarket).await.len(),
        2
    );
    assert_eq!(store.fills().await.len(), 2);
}
