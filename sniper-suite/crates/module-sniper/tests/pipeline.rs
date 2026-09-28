//! End-to-end pipeline tests: the REAL `Sniper::consider_event` against the
//! scripted mock node — every stage, every early rejection, the risk engine,
//! the hardened executor and the ledger, with no network.

mod common;

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use solana_sdk::pubkey::Pubkey;

use bot_core::events::AppEvent;
use bot_core::execution::ExecutionState;
use bot_core::models::{BotModule, PositionStatus, Venue};

use module_sniper::event::LaunchProtocol;
use module_sniper::pipeline::{EntryRoute, RejectReason, SniperStage};

use common::*;

/// Drain every audit event currently buffered on the bus.
fn audit_outcomes(
    rx: &mut tokio::sync::broadcast::Receiver<Arc<AppEvent>>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let AppEvent::Audit {
            action, outcome, ..
        } = ev.as_ref()
        {
            out.push((action.clone(), outcome.clone()));
        }
    }
    out
}

#[tokio::test]
async fn paper_entry_walks_the_full_lifecycle_and_books_a_position() {
    let mut w = World::new(base_config()).await;
    let mut events = w.state.events.subscribe();
    let event = w.event(1);
    let outcome = w.sniper.consider_event(event.clone()).await;

    assert!(outcome.rejection.is_none(), "{:?}", outcome.rejection);
    assert_eq!(outcome.stage, SniperStage::Confirmed, "paper fill confirms");
    assert!(outcome.accepted());
    assert_eq!(outcome.route, Some(EntryRoute::PumpCurve));
    let intent = outcome.intent_id.clone().expect("intent id");
    assert!(intent.starts_with("int_"));
    assert_eq!(
        Some(intent.clone()),
        module_sniper::entry::entry_intent_id(&event, EntryRoute::PumpCurve),
        "the published intent id is the deterministic one"
    );
    assert_eq!(outcome.slippage_bps, Some(1_500));
    assert!(outcome.price_impact_bps.unwrap() > 0);
    assert!(outcome.gates.starts_with("pool_state=pass"));
    // Every latency span is measured.
    let tl = &outcome.timeline;
    assert!(tl.validation_ms().is_some());
    assert!(tl.risk_ms().is_some());
    assert!(tl.build_ms().is_some());
    assert!(tl.submission_ms().is_some());
    assert!(tl.confirmation_ms().is_some());
    assert!(tl.total_ms().is_some());

    // The position exists, on the curve venue, with the curve as market id.
    let positions = w.state.open_positions_for(BotModule::Sniper).await;
    assert_eq!(positions.len(), 1);
    let p = &positions[0];
    assert_eq!(p.symbol, w.mint.to_string());
    assert_eq!(p.venue, Venue::PumpFun);
    assert_eq!(
        p.market_id.as_deref(),
        Some(
            solana_kit::pump::bonding_curve_pda(&w.mint)
                .to_string()
                .as_str()
        )
    );
    assert!(p.qty > 0.0 && p.avg_entry > 0.0);
    assert_eq!(outcome.position_id.as_deref(), Some(p.id.as_str()));
    // The ledger holds the confirmed intent under the deterministic id.
    let rec = bot_core::execution::ledger()
        .get(&intent)
        .await
        .expect("ledger record");
    assert_eq!(rec.state, ExecutionState::Confirmed);
    assert_eq!(rec.module, "sniper");
    // One audit record with the full stage path.
    let audits = audit_outcomes(&mut events);
    let (action, text) = audits
        .iter()
        .find(|(a, _)| a.starts_with("sniper.entry."))
        .expect("audit event");
    assert_eq!(action, "sniper.entry.confirmed");
    assert!(
        text.contains("path=DETECTED>VALIDATED>RISK_APPROVED>EXECUTION_READY>SUBMITTED>CONFIRMED"),
        "{text}"
    );
    assert!(text.contains(&intent));
    // Per-token attempt was recorded (feeds the cooldown).
    assert!(w
        .state
        .last_entry_attempt(&w.mint.to_string())
        .await
        .is_some());
    assert!(w
        .state
        .last_failed_entry(&w.mint.to_string())
        .await
        .is_none());
    // Metrics registered.
    let text = bot_core::obs::metrics::global().encode();
    for name in [
        "sniper_events_total",
        "sniper_stage_total",
        "sniper_gate_results_total",
        "sniper_slippage_bps",
        "sniper_validation_latency_ms",
        "sniper_risk_latency_ms",
        "sniper_build_latency_ms",
        "sniper_submission_latency_ms",
        "sniper_confirmation_latency_ms",
        "sniper_total_latency_ms",
    ] {
        assert!(text.contains(name), "missing metric {name}");
    }
}

#[tokio::test]
async fn duplicate_event_is_rejected_by_the_authoritative_dedup() {
    let mut w = World::new(base_config()).await;
    let first = w.sniper.consider_event(w.event(1)).await;
    assert!(first.rejection.is_none(), "{:?}", first.rejection);
    // Same launch again (another feed, another sequence, same signature).
    let mut again = w.event(1);
    again.source_seq = 99;
    again.observed_at = Utc::now();
    let second = w.sniper.consider_event(again).await;
    assert_eq!(
        second.rejection.as_ref().map(|r| r.reason),
        Some(RejectReason::DuplicateEvent)
    );
    assert_eq!(second.stage, SniperStage::Rejected);
    assert_eq!(second.rejection.unwrap().stage, SniperStage::Detected);
    assert_eq!(w.state.open_positions_for(BotModule::Sniper).await.len(), 1);
    assert_eq!(
        bot_core::execution::ledger()
            .open()
            .await
            .iter()
            .filter(|r| r.symbol == w.mint.to_string())
            .count(),
        0
    );
}

#[tokio::test]
async fn malformed_and_stale_events_never_reach_the_chain() {
    let mut w = World::new(base_config()).await;
    let before = w.node.requests.load(std::sync::atomic::Ordering::SeqCst);

    let mut bad = w.event(2);
    bad.signature = Some("not-a-signature".into());
    bad.event_id = bad.compute_event_id();
    let o = w.sniper.consider_event(bad).await;
    assert_eq!(
        o.rejection.as_ref().map(|r| r.reason),
        Some(RejectReason::InvalidEvent)
    );
    assert!(o.rejection.unwrap().detail.contains("signature"));

    let mut forged = w.event(3);
    forged.event_id = "evt_forged".into();
    let o = w.sniper.consider_event(forged).await;
    assert_eq!(
        o.rejection.as_ref().map(|r| r.reason),
        Some(RejectReason::InvalidEvent)
    );

    let mut stale = w.event(4);
    stale.observed_at = Utc::now() - chrono::Duration::seconds(600);
    stale.launch.observed_at = stale.observed_at;
    let o = w.sniper.consider_event(stale).await;
    assert_eq!(
        o.rejection.as_ref().map(|r| r.reason),
        Some(RejectReason::StaleEvent)
    );

    let after = w.node.requests.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(before, after, "early rejections must not cost an RPC call");
    assert!(w
        .state
        .open_positions_for(BotModule::Sniper)
        .await
        .is_empty());
}

#[tokio::test]
async fn kill_switch_disabled_module_and_symbol_gate_short_circuit() {
    let mut w = World::new(base_config()).await;

    w.state.set_kill_switch(true, "test").await;
    let o = w.sniper.consider_event(w.event(5)).await;
    assert_eq!(
        o.rejection.as_ref().map(|r| r.reason),
        Some(RejectReason::KillSwitch)
    );
    w.state.set_kill_switch(false, "test").await;

    w.state.set_enabled(BotModule::Sniper, false).await;
    let o = w.sniper.consider_event(w.event(6)).await;
    assert_eq!(
        o.rejection.as_ref().map(|r| r.reason),
        Some(RejectReason::StrategyDisabled)
    );
    w.state.set_enabled(BotModule::Sniper, true).await;

    w.state
        .update_config(|c| c.risk.sniper_emergency_disable = true)
        .await;
    let o = w.sniper.consider_event(w.event(7)).await;
    assert_eq!(
        o.rejection.as_ref().map(|r| r.reason),
        Some(RejectReason::StrategyDisabled)
    );
    assert!(o
        .rejection
        .unwrap()
        .detail
        .contains("sniper_emergency_disable"));
    w.state
        .update_config(|c| c.risk.sniper_emergency_disable = false)
        .await;

    w.state.block_symbol(&w.mint.to_string()).await;
    let o = w.sniper.consider_event(w.event(8)).await;
    assert_eq!(
        o.rejection.as_ref().map(|r| r.reason),
        Some(RejectReason::SymbolGated)
    );
    w.state.unblock_symbol(&w.mint.to_string()).await;

    // None of the above consumed the dedup slot: the launch still trades.
    let o = w.sniper.consider_event(w.event(9)).await;
    assert!(o.rejection.is_none(), "{:?}", o.rejection);
}

#[tokio::test]
async fn screening_denylist_rejects_with_risk_rejected() {
    let mut cfg = base_config();
    cfg.sniper.keyword_denylist = vec!["harness".into()];
    let mut w = World::new(cfg).await;
    let o = w.sniper.consider_event(w.event(10)).await;
    let r = o.rejection.expect("rejected");
    assert_eq!(r.reason, RejectReason::RiskRejected);
    assert!(r.detail.starts_with("screening:"), "{}", r.detail);
    assert_eq!(r.stage, SniperStage::Detected);
}

#[tokio::test]
async fn invalid_route_when_no_venue_can_execute_the_protocol() {
    let mut cfg = base_config();
    cfg.sniper.trade_raydium = false;
    cfg.sniper.use_jupiter_fallback = false;
    let mut w = World::new(cfg).await;
    let mut ev = w.event(11);
    ev.protocol = LaunchProtocol::RaydiumAmmV4;
    ev.pool = Some(Pubkey::new_unique().to_string());
    ev.event_id = ev.compute_event_id();
    let o = w.sniper.consider_event(ev).await;
    assert_eq!(
        o.rejection.as_ref().map(|r| r.reason),
        Some(RejectReason::InvalidRoute)
    );
}

#[tokio::test]
async fn graduated_curve_without_a_route_is_invalid_route_and_pool_not_ready_when_missing() {
    // Curve complete, PumpSwap and Jupiter both off → INVALID_ROUTE.
    let mut cfg = base_config();
    cfg.sniper.trade_pumpswap = false;
    cfg.sniper.use_jupiter_fallback = false;
    let mut w = World::new(cfg).await;
    let mut spec = CurveSpec::fresh();
    spec.complete = true;
    w.node.set_account(
        solana_kit::pump::bonding_curve_pda(&w.mint),
        *solana_kit::consts::PUMP_PROGRAM_ID,
        bonding_curve_bytes(w.creator, spec),
    );
    let o = w.sniper.consider_event(w.event(12)).await;
    assert_eq!(
        o.rejection.as_ref().map(|r| r.reason),
        Some(RejectReason::InvalidRoute)
    );
    assert_eq!(o.stage, SniperStage::Rejected);

    // Curve account missing entirely → POOL_NOT_READY.
    let other = Pubkey::new_unique();
    w.node.set_account(
        other,
        *solana_kit::consts::TOKEN_PROGRAM,
        mint_bytes(None, None, 1_000, 6),
    );
    let o = w
        .sniper
        .consider_event(pump_event(other, w.creator, 13))
        .await;
    assert_eq!(
        o.rejection.as_ref().map(|r| r.reason),
        Some(RejectReason::PoolNotReady)
    );
}

#[tokio::test]
async fn token_state_and_liquidity_gates_reject_with_their_reasons() {
    let mut w = World::new(base_config()).await;
    // Freeze authority still set (default gate on).
    w.node.set_account(
        w.mint,
        *solana_kit::consts::TOKEN_PROGRAM,
        mint_bytes(None, Some(Pubkey::new_unique()), 1_000, 6),
    );
    let o = w.sniper.consider_event(w.event(14)).await;
    let r = o.rejection.expect("rejected");
    assert_eq!(r.reason, RejectReason::TokenStateInvalid);
    assert!(r.detail.contains("freeze_authority"), "{}", r.detail);
    assert!(o.gates.contains("freeze_authority=fail"));

    // Liquidity threshold on a second token: 0.5 SOL real < 5 SOL minimum.
    w.state
        .update_config(|c| c.sniper.min_liquidity_sol = 5.0)
        .await;
    let mint2 = Pubkey::new_unique();
    install_pump_token(&w.node, mint2, w.creator, CurveSpec::fresh());
    let o = w
        .sniper
        .consider_event(pump_event(mint2, w.creator, 15))
        .await;
    assert_eq!(
        o.rejection.as_ref().map(|r| r.reason),
        Some(RejectReason::InsufficientLiquidity)
    );
}

#[tokio::test]
async fn slippage_and_price_impact_limits_reject_before_risk() {
    // 0.05 SOL into a 30.5 SOL virtual reserve ≈ 16 bps impact; a 10 bps
    // cap trips PRICE_IMPACT_LIMIT.
    let mut cfg = base_config();
    cfg.sniper.max_price_impact_bps = 10;
    let mut w = World::new(cfg).await;
    let o = w.sniper.consider_event(w.event(16)).await;
    assert_eq!(
        o.rejection.as_ref().map(|r| r.reason),
        Some(RejectReason::PriceImpactLimit)
    );
    assert!(o.price_impact_bps.unwrap() > 10);

    // Adaptive mode against a hard max below the impact → SLIPPAGE_LIMIT.
    w.state
        .update_config(|c| {
            c.sniper.max_price_impact_bps = 0;
            c.sniper.slippage_mode = "price_impact".into();
            c.risk.max_slippage_bps = 5;
        })
        .await;
    let mint2 = Pubkey::new_unique();
    install_pump_token(&w.node, mint2, w.creator, CurveSpec::fresh());
    let o = w
        .sniper
        .consider_event(pump_event(mint2, w.creator, 17))
        .await;
    assert_eq!(
        o.rejection.as_ref().map(|r| r.reason),
        Some(RejectReason::SlippageLimit)
    );

    // Fixed mode above the hard max is refused by the RISK ENGINE (one
    // authority), reported under the same reason.
    w.state
        .update_config(|c| {
            c.sniper.slippage_mode = "fixed".into();
            c.sniper.slippage_pct = 15.0;
            c.risk.max_slippage_bps = 100;
        })
        .await;
    let mint3 = Pubkey::new_unique();
    install_pump_token(&w.node, mint3, w.creator, CurveSpec::fresh());
    let o = w
        .sniper
        .consider_event(pump_event(mint3, w.creator, 18))
        .await;
    let r = o.rejection.expect("rejected");
    assert_eq!(r.reason, RejectReason::SlippageLimit);
    assert!(r.detail.contains("risk[slippage_cap]"), "{}", r.detail);
}

#[tokio::test]
async fn risk_exposure_limits_map_to_exposure_limit() {
    let mut cfg = base_config();
    cfg.risk.sniper_max_concurrent_positions = 1;
    let mut w = World::new(cfg).await;
    let first = w.sniper.consider_event(w.event(19)).await;
    assert!(first.rejection.is_none(), "{:?}", first.rejection);
    let mint2 = Pubkey::new_unique();
    install_pump_token(&w.node, mint2, w.creator, CurveSpec::fresh());
    let o = w
        .sniper
        .consider_event(pump_event(mint2, w.creator, 20))
        .await;
    let r = o.rejection.expect("rejected");
    assert_eq!(r.reason, RejectReason::ExposureLimit);
    assert!(r.detail.contains("max_open_positions"), "{}", r.detail);
    assert_eq!(r.stage, SniperStage::Validated);
    assert_eq!(
        w.state
            .module_status(BotModule::Sniper)
            .await
            .orders_rejected_by_risk,
        1
    );
}

#[tokio::test]
async fn failed_entry_cooldown_refuses_the_same_mint_after_a_rejected_send() {
    let mut cfg = live_config();
    cfg.risk.sniper_failed_entry_cooldown_secs = 300;
    let mut w = World::new(cfg).await;
    w.node.set_send(SendBehaviour::Reject);
    let o = w.sniper.consider_event(w.event(21)).await;
    assert_eq!(o.stage, SniperStage::Failed);
    assert_eq!(
        o.rejection.as_ref().map(|r| r.reason),
        Some(RejectReason::ExecutionUnavailable)
    );
    assert!(o.infra_failure());
    assert!(o.rejection.as_ref().unwrap().detail.contains("SendFailed"));
    assert!(w
        .state
        .last_failed_entry(&w.mint.to_string())
        .await
        .is_some());
    assert!(w
        .state
        .open_positions_for(BotModule::Sniper)
        .await
        .is_empty());

    // A second launch of the same mint (dedup key is the mint) is a
    // duplicate; a different mint is fine, proving the cooldown is per mint.
    w.node.set_send(SendBehaviour::Accept);
    let mint2 = Pubkey::new_unique();
    install_pump_token(&w.node, mint2, w.creator, CurveSpec::fresh());
    let o = w
        .sniper
        .consider_event(pump_event(mint2, w.creator, 22))
        .await;
    assert!(o.rejection.is_none(), "{:?}", o.rejection);
    // And the risk engine reports the cooldown for the failed mint directly.
    let d = w
        .sniper
        .risk()
        .check_entry(&bot_core::risk::EntryRequest {
            module: BotModule::Sniper,
            venue: Venue::PumpFun,
            symbol: w.mint.to_string(),
            symbol_display: "HRN".into(),
            requested_quote: 0.05,
            available_quote: 10.0,
            slippage_bps: 1_500,
            price: None,
            fair_value: None,
            liquidity: None,
            wallet: String::new(),
            strategy: String::new(),
        })
        .await;
    assert_eq!(
        d.code,
        Some(bot_core::risk::RiskCode::SniperFailedEntryCooldown)
    );
}

#[tokio::test]
async fn live_entry_confirms_through_the_hardened_executor() {
    let mut w = World::new(live_config()).await;
    let o = w.sniper.consider_event(w.event(23)).await;
    assert!(o.rejection.is_none(), "{:?}", o.rejection);
    assert_eq!(o.stage, SniperStage::Confirmed);
    assert_eq!(w.node.sends.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(w.node.simulations.load(std::sync::atomic::Ordering::SeqCst) >= 1);
    let intent = o.intent_id.unwrap();
    let rec = bot_core::execution::ledger().get(&intent).await.unwrap();
    assert_eq!(rec.state, ExecutionState::Confirmed);
    assert_eq!(rec.signature, w.node.last_sent_signature());
    let p = &w.state.open_positions_for(BotModule::Sniper).await[0];
    assert_eq!(p.entry_signature, w.node.last_sent_signature());
    assert_eq!(p.status, PositionStatus::Open);
}

#[tokio::test]
async fn latency_budget_is_enforced_right_before_submission() {
    let mut cfg = base_config();
    cfg.sniper.max_entry_latency_ms = 50;
    let mut w = World::new(cfg).await;
    // Every RPC answer takes 60 ms, so by the time the transaction is built
    // the event has been held far longer than the 50 ms budget.
    w.node
        .latency_ms
        .store(60, std::sync::atomic::Ordering::SeqCst);
    let o = w.sniper.consider_event(w.event(24)).await;
    let r = o.rejection.expect("rejected");
    assert_eq!(r.reason, RejectReason::StaleEvent);
    assert!(r.detail.contains("entry latency"), "{}", r.detail);
    assert_eq!(r.stage, SniperStage::RiskApproved);
    assert_eq!(w.node.sends.load(std::sync::atomic::Ordering::SeqCst), 0);
    // The attempt was never counted (nothing left the process).
    assert!(w
        .state
        .last_entry_attempt(&w.mint.to_string())
        .await
        .is_none());
    tokio::time::sleep(Duration::from_millis(10)).await;
}

#[tokio::test]
async fn fee_budget_and_fee_policy_reject_with_fee_limit_before_risk() {
    // base_config: fixed policy, 250 000 µlamports/CU × 400 000 CU, one
    // attempt → 100 000 + 5 000 base = 105 000 lamports worst case.
    let mut cfg = base_config();
    cfg.sniper.max_entry_fee_lamports = 104_999;
    let mut w = World::new(cfg).await;
    let mut events = w.state.events.subscribe();
    let o = w.sniper.consider_event(w.event(25)).await;
    let r = o.rejection.clone().expect("rejected");
    assert_eq!(r.reason, RejectReason::FeeLimit);
    assert_eq!(r.stage, SniperStage::Validated, "refused before risk");
    assert!(r.detail.contains("105000 lamports"), "{}", r.detail);
    assert!(
        r.detail.contains("max_entry_fee_lamports 104999"),
        "{}",
        r.detail
    );
    assert_eq!(o.route, Some(EntryRoute::PumpCurve));
    assert_eq!(
        o.fee_estimate_lamports, None,
        "an estimate over budget is not reported as accepted"
    );
    // Nothing was attempted or counted against the strategy: no risk
    // rejection, no failed order, no attempt timestamp, no ledger record.
    let status = w.state.module_status(BotModule::Sniper).await;
    assert_eq!(status.orders_rejected_by_risk, 0);
    assert_eq!(status.orders_failed, 0);
    assert!(w
        .state
        .last_entry_attempt(&w.mint.to_string())
        .await
        .is_none());
    let records = bot_core::execution::ledger().list(10_000).await;
    assert!(!records.iter().any(|rec| rec.symbol == w.mint.to_string()));
    assert_eq!(w.node.sends.load(std::sync::atomic::Ordering::SeqCst), 0);
    let audits = audit_outcomes(&mut events);
    assert!(
        audits
            .iter()
            .any(|(a, out)| a == "sniper.entry.rejected" && out.contains("reason=FEE_LIMIT")),
        "{audits:?}"
    );

    // A budget of exactly the estimate lets the next launch through, and
    // the outcome / audit trail carry the number it was budgeted at.
    w.state
        .update_config(|c| c.sniper.max_entry_fee_lamports = 105_000)
        .await;
    let mint2 = Pubkey::new_unique();
    install_pump_token(&w.node, mint2, w.creator, CurveSpec::fresh());
    let o = w
        .sniper
        .consider_event(pump_event(mint2, w.creator, 26))
        .await;
    assert!(o.rejection.is_none(), "{:?}", o.rejection);
    assert_eq!(o.fee_estimate_lamports, Some(105_000));
    let audits = audit_outcomes(&mut events);
    assert!(
        audits
            .iter()
            .any(|(_, out)| out.contains("fee_est_lamports=105000")),
        "{audits:?}"
    );

    // A priority fee the execution engine's own policy would refuse is
    // reported here as FEE_LIMIT — before any attempt — instead of as a
    // failed submission. (The budget is off; only the policy speaks.)
    w.state
        .update_config(|c| {
            c.sniper.max_entry_fee_lamports = 0;
            c.execution.priority_fee_micro_lamports =
                c.execution.fee_emergency_max_micro_lamports + 1;
        })
        .await;
    let mint3 = Pubkey::new_unique();
    install_pump_token(&w.node, mint3, w.creator, CurveSpec::fresh());
    let o = w
        .sniper
        .consider_event(pump_event(mint3, w.creator, 27))
        .await;
    let r = o.rejection.expect("rejected");
    assert_eq!(r.reason, RejectReason::FeeLimit);
    assert_eq!(r.stage, SniperStage::Validated);
    assert!(r.detail.contains("emergency limit"), "{}", r.detail);
    let status = w.state.module_status(BotModule::Sniper).await;
    assert_eq!(status.orders_failed, 0, "no failed submission was recorded");
    assert!(w
        .state
        .last_entry_attempt(&mint3.to_string())
        .await
        .is_none());
    tokio::time::sleep(Duration::from_millis(10)).await;
}

#[tokio::test]
async fn legacy_consider_launch_still_trades_a_token_launch() {
    let mut w = World::new(base_config()).await;
    let launch = token_launch(w.mint, w.creator, &signature(25), Utc::now());
    w.sniper.consider_launch(launch).await.expect("legacy path");
    assert_eq!(w.state.open_positions_for(BotModule::Sniper).await.len(), 1);
}
