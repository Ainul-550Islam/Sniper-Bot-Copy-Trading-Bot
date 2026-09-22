//! Policy and sizing (TASK 3 test 15): precedence of the policy rules, the
//! sizing arithmetic against every configured cap, degenerate inputs, and
//! how the approved size flows into the booked position.

mod common;

use common::*;

use bot_core::config::CopyConfig;
use bot_core::models::{BotModule, PositionSide, Venue};
use module_copy::event::{CopyStage, EventSource, RejectReason};
use module_copy::leader::LeaderStatus;
use module_copy::policy::{self, PolicyContext, PolicyVerdict};
use module_copy::sizing::{self, SizeClamp, SizingError, SizingMode};

fn reason(v: &PolicyVerdict) -> Option<RejectReason> {
    v.rejection().map(|r| r.reason)
}

#[test]
fn policy_precedence_table() {
    let cfg = CopyConfig::default();
    let mint = solana_sdk::pubkey::Pubkey::new_unique();
    let e = event(
        &leader_buy(LEADER, mint, 1, 1.0),
        EventSource::PumpPortal,
        1,
    );
    let r = rule(LEADER);
    let now = chrono::Utc::now();
    let clean = PolicyContext::clean(now);

    // Every failing condition at once: the FIRST rule in the documented
    // order wins, and removing it exposes the next.
    let mut stale = e.clone();
    stale.observed_at = ago(1_000);
    stale.block_time = Some(ago(1_000));
    let mut tiny_rule = r.clone();
    tiny_rule.min_sol = 5.0;
    let mut no_venue = cfg.clone();
    no_venue.decode_pumpfun = false;
    no_venue.skip_if_sniper_holds = true;
    let everything = PolicyContext {
        symbol_blocked: true,
        sniper_holds: true,
        holding: true,
        held_qty: 1.0,
        ..clean
    };
    let mut replay = stale.clone();
    replay.source = EventSource::Replay;

    assert_eq!(
        reason(&policy::evaluate(&replay, None, &no_venue, &everything)),
        Some(RejectReason::LeaderUnknown)
    );
    assert_eq!(
        reason(&policy::evaluate(
            &replay,
            Some((&tiny_rule, LeaderStatus::Removed)),
            &no_venue,
            &everything
        )),
        Some(RejectReason::LeaderRemoved)
    );
    assert_eq!(
        reason(&policy::evaluate(
            &replay,
            Some((&tiny_rule, LeaderStatus::Paused)),
            &no_venue,
            &everything
        )),
        Some(RejectReason::ReplayOnly),
        "replay is decided before the pause (it applies to sells too)"
    );
    assert_eq!(
        reason(&policy::evaluate(
            &stale,
            Some((&tiny_rule, LeaderStatus::Paused)),
            &no_venue,
            &everything
        )),
        Some(RejectReason::LeaderPaused)
    );
    assert_eq!(
        reason(&policy::evaluate(
            &stale,
            Some((&tiny_rule, LeaderStatus::Active)),
            &no_venue,
            &everything
        )),
        Some(RejectReason::VenueDisabled)
    );
    no_venue.decode_pumpfun = true;
    assert_eq!(
        reason(&policy::evaluate(
            &stale,
            Some((&tiny_rule, LeaderStatus::Active)),
            &no_venue,
            &everything
        )),
        Some(RejectReason::BelowLeaderMin)
    );
    assert_eq!(
        reason(&policy::evaluate(
            &stale,
            Some((&r, LeaderStatus::Active)),
            &no_venue,
            &everything
        )),
        Some(RejectReason::StaleEvent)
    );
    assert_eq!(
        reason(&policy::evaluate(
            &e,
            Some((&r, LeaderStatus::Active)),
            &no_venue,
            &everything
        )),
        Some(RejectReason::SymbolGated)
    );
    let unblocked = PolicyContext {
        symbol_blocked: false,
        ..everything
    };
    assert_eq!(
        reason(&policy::evaluate(
            &e,
            Some((&r, LeaderStatus::Active)),
            &no_venue,
            &unblocked
        )),
        Some(RejectReason::SniperHolds)
    );
    let no_sniper = PolicyContext {
        sniper_holds: false,
        ..unblocked
    };
    assert_eq!(
        reason(&policy::evaluate(
            &e,
            Some((&r, LeaderStatus::Active)),
            &no_venue,
            &no_sniper
        )),
        Some(RejectReason::AlreadyMirroring)
    );
    match policy::evaluate(&e, Some((&r, LeaderStatus::Active)), &no_venue, &clean) {
        PolicyVerdict::Enter(plan) => {
            assert_eq!(
                plan.slippage_pct, 15.0,
                "rule override wins over [copy].slippage_pct"
            );
            assert_eq!(plan.rule.address, LEADER);
        }
        other => panic!("expected Enter, got {other:?}"),
    }
    // Every documented reason label is SCREAMING_SNAKE and unique.
    for reason in RejectReason::ALL {
        assert!(reason
            .as_str()
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '_'));
    }
}

#[test]
fn sizing_matrix() {
    let cfg = CopyConfig::default();
    let mut r = rule(LEADER); // fixed 0.05, max_sol 1.0
    let d = sizing::size_mirror(&r, &cfg, 3.0, Some(10.0)).unwrap();
    assert_eq!(d.mode, SizingMode::Fixed);
    assert_eq!(d.requested_sol, 0.05);
    assert!(d.clamps.is_empty());

    r.fixed_sol = None;
    r.fraction_of_their_size = 0.2;
    let d = sizing::size_mirror(&r, &cfg, 3.0, Some(10.0)).unwrap();
    assert_eq!(d.mode, SizingMode::Proportional);
    assert!((d.requested_sol - 0.6).abs() < 1e-12);
    assert!((d.ratio() - 0.2).abs() < 1e-12);

    // Rule cap, then global per-trade cap, then balance fraction — each
    // recorded only when it actually reduced the size.
    let d = sizing::size_mirror(&r, &cfg, 10.0, Some(10.0)).unwrap();
    assert_eq!(d.requested_sol, 1.0);
    assert_eq!(d.clamps, vec![SizeClamp::RuleMaxSol]);
    let mut capped = cfg.clone();
    capped.max_sol_per_trade = 0.5;
    let d = sizing::size_mirror(&r, &capped, 10.0, Some(10.0)).unwrap();
    assert_eq!(d.requested_sol, 0.5);
    assert_eq!(
        d.clamps,
        vec![SizeClamp::RuleMaxSol, SizeClamp::GlobalMaxPerTrade]
    );
    capped.max_balance_fraction = 0.02;
    let d = sizing::size_mirror(&r, &capped, 10.0, Some(10.0)).unwrap();
    assert!((d.requested_sol - 0.2).abs() < 1e-12);
    assert_eq!(
        d.clamps,
        vec![
            SizeClamp::RuleMaxSol,
            SizeClamp::GlobalMaxPerTrade,
            SizeClamp::BalanceFraction
        ]
    );
    // A cap that is not binding leaves no trace.
    let d = sizing::size_mirror(&r, &capped, 0.5, Some(10.0)).unwrap();
    assert!((d.requested_sol - 0.1).abs() < 1e-12);
    assert!(d.clamps.is_empty());

    // Degenerate inputs.
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            sizing::size_mirror(&r, &cfg, bad, None),
            Err(SizingError::NonFinite)
        );
    }
    assert_eq!(
        sizing::size_mirror(&r, &cfg, 0.0, None),
        Err(SizingError::NonPositive)
    );
    assert_eq!(
        sizing::size_mirror(&r, &cfg, -3.0, None),
        Err(SizingError::NonPositive)
    );
    let mut zero_balance = capped.clone();
    zero_balance.max_balance_fraction = 0.5;
    assert_eq!(
        sizing::size_mirror(&r, &zero_balance, 1.0, Some(0.0)),
        Err(SizingError::NonPositive),
        "no balance → nothing to spend"
    );
    let mut dusty = cfg.clone();
    dusty.min_mirror_sol = 0.25;
    assert!(matches!(
        sizing::size_mirror(&r, &dusty, 1.0, None),
        Err(SizingError::Dust { floor, .. }) if floor == 0.25
    ));
    // The pre-TASK-3 helper still answers the same base size.
    let trade = leader_buy(LEADER, solana_sdk::pubkey::Pubkey::new_unique(), 2, 3.0);
    assert!((module_copy::mirror::size_for(&r, &trade) - 0.6).abs() < 1e-12);
}

#[tokio::test]
async fn approved_size_flows_into_the_position_and_risk_may_reduce_it() {
    let mut cfg = copy_config();
    cfg.copy.wallets[0].fixed_sol = None;
    cfg.copy.wallets[0].fraction_of_their_size = 0.5;
    cfg.copy.wallets[0].max_sol = 10.0;
    cfg.risk.max_position_quote = 0.3;
    cfg.risk.copy_max_position_quote = 0.2;
    let mut w = CopyWorld::new(cfg.clone()).await;

    // Leader buys 0.5 SOL → proportional 0.25 → risk engine caps a copy entry
    // at 0.2 (copy_max_position_quote) and allows the reduced size.
    let out = w.process(&w.buy(1)).await;
    assert_eq!(out.stage, CopyStage::Filled, "{:?}", out.rejection);
    assert_eq!(out.requested_sol, Some(0.25));
    assert_eq!(out.sized_sol, Some(0.2));
    let p = &w.state.open_positions_for(BotModule::Copy).await[0];
    assert!(
        (p.cost_basis - 0.2).abs() < 1e-9,
        "cost basis = approved size, got {}",
        p.cost_basis
    );

    // Global per-trade cap in [copy] applies before the risk engine sees it.
    let mut cfg2 = cfg.clone();
    cfg2.copy.max_sol_per_trade = 0.1;
    w.reload(cfg2.clone()).await;
    let m = w.new_token();
    let out = w.process(&w.buy_in(m, 2)).await;
    assert_eq!(out.stage, CopyStage::Filled, "{:?}", out.rejection);
    assert_eq!(out.requested_sol, Some(0.1));
    assert_eq!(out.sized_sol, Some(0.1));

    // Balance fraction: 1% of the 10 SOL paper balance.
    let mut cfg3 = cfg2.clone();
    cfg3.copy.max_sol_per_trade = 0.0;
    cfg3.copy.max_balance_fraction = 0.01;
    w.reload(cfg3).await;
    let m = w.new_token();
    let out = w.process(&w.buy_in(m, 3)).await;
    assert_eq!(out.stage, CopyStage::Filled, "{:?}", out.rejection);
    assert!((out.requested_sol.unwrap() - 0.1).abs() < 1e-9);
}

#[tokio::test]
async fn sells_bypass_sizing_and_use_the_exit_fraction() {
    let mut cfg = copy_config();
    cfg.copy.full_exit_on_their_exit = false;
    let mut w = CopyWorld::new(cfg).await;
    assert_eq!(w.process(&w.buy(1)).await.stage, CopyStage::Filled);
    let held = w
        .state
        .find_open(BotModule::Copy, &w.mint.to_string())
        .await
        .unwrap();
    // Leader sells a quarter of what we hold → we sell a quarter.
    let mut partial = leader_sell(LEADER, w.mint, 2, held.qty * 0.25);
    partial.side = PositionSide::Short;
    partial.venue = Venue::PumpFun;
    let out = w
        .process(&event(&partial, EventSource::PumpPortal, 2))
        .await;
    assert_eq!(out.stage, CopyStage::ExitMirrored, "{:?}", out.rejection);
    let after = w
        .state
        .find_open(BotModule::Copy, &w.mint.to_string())
        .await
        .expect("still open");
    assert!(
        (after.qty / held.qty - 0.75).abs() < 1e-6,
        "{} vs {}",
        after.qty,
        held.qty * 0.75
    );
    // A sell larger than our holding closes fully.
    let big = leader_sell(LEADER, w.mint, 3, held.qty * 10.0);
    let out = w.process(&event(&big, EventSource::PumpPortal, 3)).await;
    assert_eq!(out.stage, CopyStage::ExitMirrored, "{:?}", out.rejection);
    assert!(w
        .state
        .find_open(BotModule::Copy, &w.mint.to_string())
        .await
        .is_none());
}
