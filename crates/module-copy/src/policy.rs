//! Copy policy (TASK 3 §05): *should this leader event be mirrored at all?*
//!
//! Pure decision over the event, the leader's registry entry, `[copy]` and a
//! small [`PolicyContext`] the caller fills from shared state. It answers
//! one of three things — mirror the entry, mirror the exit, or skip with a
//! machine-readable [`Rejection`] — and it never sizes, never talks to the
//! risk engine and never touches I/O. Sizing is `sizing.rs`; the one
//! authoritative risk decision stays with `bot_core::risk::RiskEngine`.
//!
//! Check order for a leader **buy** (first failing rule wins):
//!
//! 1. leader known / not removed / not paused
//! 2. source may execute (replays never trade)
//! 3. venue decoder enabled (`decode_pumpfun` / `decode_pumpswap` /
//!    `decode_raydium` / `decode_jupiter`)
//! 4. leader size ≥ the wallet rule's `min_sol`
//! 5. staleness — the wallet rule's `max_staleness_secs` (since we observed
//!    it) and the global `max_event_age_secs` (since chain time)
//! 6. symbol not gated by unresolved reconciliation
//! 7. `skip_if_sniper_holds`
//! 8. not already mirroring the mint
//!
//! For a leader **sell**: leader known / not removed (a *paused* leader's
//! exits are still mirrored — reducing exposure is always allowed), source
//! may execute, `mirror_exits` on, rule not `buys_only`, and we hold the
//! mint. The exit fraction follows `full_exit_on_their_exit`.

use bot_core::config::{CopyConfig, CopyWallet};
use bot_core::models::Venue;
use chrono::{DateTime, Utc};

use crate::event::{CopyStage, LeaderTradeEvent, RejectReason, Rejection};
use crate::leader::LeaderStatus;

/// Facts the policy needs that live in shared state.
#[derive(Debug, Clone, Copy)]
pub struct PolicyContext {
    /// Evaluation time.
    pub now: DateTime<Utc>,
    /// `AppState::is_symbol_blocked(mint)`.
    pub symbol_blocked: bool,
    /// The sniper holds an open position in the mint.
    pub sniper_holds: bool,
    /// The copy module holds an open position in the mint.
    pub holding: bool,
    /// Quantity of that copy position (for proportional exits; `0` = none).
    pub held_qty: f64,
}

impl PolicyContext {
    /// Context with no positions and no gates, evaluated now.
    pub fn clean(now: DateTime<Utc>) -> Self {
        PolicyContext {
            now,
            symbol_blocked: false,
            sniper_holds: false,
            holding: false,
            held_qty: 0.0,
        }
    }
}

/// How to mirror an entry.
#[derive(Debug, Clone, PartialEq)]
pub struct EntryPlan {
    /// The wallet rule that applies.
    pub rule: CopyWallet,
    /// Slippage to use (rule override, else `[copy].slippage_pct`).
    pub slippage_pct: f64,
    /// Seconds since we observed the event.
    pub observed_age_secs: i64,
    /// Seconds since the event's chain time (or observation when unknown).
    pub event_age_secs: i64,
}

/// How to mirror an exit.
#[derive(Debug, Clone, PartialEq)]
pub struct ExitPlan {
    /// The wallet rule that applies.
    pub rule: CopyWallet,
    /// Fraction of our position to sell (`1.0` = full close).
    pub fraction: f64,
}

/// The policy's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum PolicyVerdict {
    /// Mirror the leader's buy.
    Enter(EntryPlan),
    /// Mirror the leader's sell.
    Exit(ExitPlan),
    /// Do nothing (reason attached).
    Skip(Rejection),
}

impl PolicyVerdict {
    /// The rejection when the verdict is a skip.
    pub fn rejection(&self) -> Option<&Rejection> {
        match self {
            PolicyVerdict::Skip(r) => Some(r),
            _ => None,
        }
    }
}

/// Whether `[copy]` decodes (and therefore mirrors) trades on `venue`.
pub fn venue_enabled(cfg: &CopyConfig, venue: Venue) -> bool {
    match venue {
        Venue::PumpFun => cfg.decode_pumpfun,
        Venue::PumpSwap => cfg.decode_pumpswap,
        Venue::RaydiumAmmV4 | Venue::RaydiumClmm => cfg.decode_raydium,
        Venue::Jupiter => cfg.decode_jupiter,
        Venue::Paper => true,
        Venue::PolymarketClob => false,
    }
}

/// Evaluate the policy.
pub fn evaluate(
    event: &LeaderTradeEvent,
    leader: Option<(&CopyWallet, LeaderStatus)>,
    cfg: &CopyConfig,
    ctx: &PolicyContext,
) -> PolicyVerdict {
    let skip = |reason: RejectReason, detail: String| {
        PolicyVerdict::Skip(Rejection::new(reason, CopyStage::LeaderResolved, detail))
    };
    let Some((rule, status)) = leader else {
        return skip(
            RejectReason::LeaderUnknown,
            format!("{} is not a followed leader", event.leader),
        );
    };
    if status == LeaderStatus::Removed {
        return skip(
            RejectReason::LeaderRemoved,
            format!("{} was unfollowed", event.leader),
        );
    }
    if !event.source.may_execute() {
        return PolicyVerdict::Skip(Rejection::new(
            RejectReason::ReplayOnly,
            CopyStage::PolicyPassed,
            format!("source {} never trades", event.source.as_str()),
        ));
    }

    if !event.is_buy() {
        return evaluate_exit(event, rule, cfg, ctx);
    }

    if status == LeaderStatus::Paused {
        return skip(
            RejectReason::LeaderPaused,
            format!("{} is paused", event.leader),
        );
    }
    let policy = |reason: RejectReason, detail: String| {
        PolicyVerdict::Skip(Rejection::new(reason, CopyStage::PolicyPassed, detail))
    };
    if !venue_enabled(cfg, event.venue) {
        return policy(
            RejectReason::VenueDisabled,
            format!("decoder for {} is off", event.venue.as_str()),
        );
    }
    if event.sol_amount < rule.min_sol {
        return policy(
            RejectReason::BelowLeaderMin,
            format!(
                "leader bought {:.4} SOL, rule minimum {:.4}",
                event.sol_amount, rule.min_sol
            ),
        );
    }
    let observed_age = ctx
        .now
        .signed_duration_since(event.observed_at)
        .num_seconds();
    if rule.max_staleness_secs > 0 && observed_age > rule.max_staleness_secs {
        return policy(
            RejectReason::StaleEvent,
            format!(
                "observed {observed_age}s ago, rule max_staleness_secs {}",
                rule.max_staleness_secs
            ),
        );
    }
    let event_age = event.age_secs(ctx.now);
    if event.is_stale(ctx.now, cfg.max_event_age_secs) {
        return policy(
            RejectReason::StaleEvent,
            format!(
                "event is {event_age}s old, copy.max_event_age_secs {}",
                cfg.max_event_age_secs
            ),
        );
    }
    if ctx.symbol_blocked {
        return policy(
            RejectReason::SymbolGated,
            format!("{} has unresolved reconciliation claims", event.mint),
        );
    }
    if cfg.skip_if_sniper_holds && ctx.sniper_holds {
        return policy(
            RejectReason::SniperHolds,
            format!("sniper already holds {}", event.mint),
        );
    }
    if ctx.holding {
        return policy(
            RejectReason::AlreadyMirroring,
            format!("already holding a copy position in {}", event.mint),
        );
    }
    PolicyVerdict::Enter(EntryPlan {
        rule: rule.clone(),
        slippage_pct: rule.slippage_pct.unwrap_or(cfg.slippage_pct),
        observed_age_secs: observed_age,
        event_age_secs: event_age,
    })
}

fn evaluate_exit(
    event: &LeaderTradeEvent,
    rule: &CopyWallet,
    cfg: &CopyConfig,
    ctx: &PolicyContext,
) -> PolicyVerdict {
    let policy = |reason: RejectReason, detail: String| {
        PolicyVerdict::Skip(Rejection::new(reason, CopyStage::PolicyPassed, detail))
    };
    if !cfg.mirror_exits {
        return policy(
            RejectReason::MirrorExitsDisabled,
            "copy.mirror_exits is off".into(),
        );
    }
    if rule.buys_only {
        return policy(
            RejectReason::SellsNotMirrored,
            format!("{} is buys_only", event.leader),
        );
    }
    if !ctx.holding {
        return policy(
            RejectReason::NoPositionToExit,
            format!("leader sold {} but we do not hold it", event.mint),
        );
    }
    let fraction = if cfg.full_exit_on_their_exit {
        1.0
    } else if ctx.held_qty > 0.0 && event.token_amount > 0.0 {
        (event.token_amount / ctx.held_qty).clamp(0.0, 1.0)
    } else {
        1.0
    };
    PolicyVerdict::Exit(ExitPlan {
        rule: rule.clone(),
        fraction,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventSource;
    use bot_core::models::{PositionSide, WalletTrade};
    use chrono::Duration;

    fn event(side: PositionSide, sol: f64) -> LeaderTradeEvent {
        let now = Utc::now();
        LeaderTradeEvent::from_wallet_trade(
            &WalletTrade {
                wallet: "whale".into(),
                signature: "sig".into(),
                slot: 5,
                block_time: Some(now - Duration::seconds(1)),
                side,
                mint: "mint".into(),
                symbol: Some("M".into()),
                token_amount: 100.0,
                sol_amount: sol,
                venue: Venue::PumpFun,
                fee_sol: 0.0,
                discriminator: None,
                observed_at: now,
            },
            EventSource::PumpPortal,
            1,
        )
    }

    fn rule() -> CopyWallet {
        CopyWallet {
            address: "whale".into(),
            min_sol: 0.1,
            max_staleness_secs: 20,
            ..CopyWallet::default()
        }
    }

    fn reason(v: &PolicyVerdict) -> Option<RejectReason> {
        v.rejection().map(|r| r.reason)
    }

    #[test]
    fn buy_precedence_first_failing_rule_wins() {
        let cfg = CopyConfig::default();
        let e = event(PositionSide::Long, 1.0);
        let ctx = PolicyContext::clean(Utc::now());
        let r = rule();
        assert_eq!(
            reason(&evaluate(&e, None, &cfg, &ctx)),
            Some(RejectReason::LeaderUnknown)
        );
        assert_eq!(
            reason(&evaluate(&e, Some((&r, LeaderStatus::Removed)), &cfg, &ctx)),
            Some(RejectReason::LeaderRemoved)
        );
        assert_eq!(
            reason(&evaluate(&e, Some((&r, LeaderStatus::Paused)), &cfg, &ctx)),
            Some(RejectReason::LeaderPaused)
        );
        let mut replay = e.clone();
        replay.source = EventSource::Replay;
        assert_eq!(
            reason(&evaluate(
                &replay,
                Some((&r, LeaderStatus::Active)),
                &cfg,
                &ctx
            )),
            Some(RejectReason::ReplayOnly)
        );
        let mut off = cfg.clone();
        off.decode_pumpfun = false;
        assert_eq!(
            reason(&evaluate(&e, Some((&r, LeaderStatus::Active)), &off, &ctx)),
            Some(RejectReason::VenueDisabled)
        );
        let small = event(PositionSide::Long, 0.05);
        assert_eq!(
            reason(&evaluate(
                &small,
                Some((&r, LeaderStatus::Active)),
                &cfg,
                &ctx
            )),
            Some(RejectReason::BelowLeaderMin)
        );
        let late = PolicyContext::clean(Utc::now() + Duration::seconds(25));
        assert_eq!(
            reason(&evaluate(&e, Some((&r, LeaderStatus::Active)), &cfg, &late)),
            Some(RejectReason::StaleEvent)
        );
        let mut loose = r.clone();
        loose.max_staleness_secs = 0;
        let mut tight = cfg.clone();
        tight.max_event_age_secs = 10;
        let later = PolicyContext::clean(Utc::now() + Duration::seconds(15));
        let v = evaluate(&e, Some((&loose, LeaderStatus::Active)), &tight, &later);
        assert_eq!(reason(&v), Some(RejectReason::StaleEvent));
        assert!(v.rejection().unwrap().detail.contains("max_event_age_secs"));
        let gated = PolicyContext {
            symbol_blocked: true,
            ..ctx
        };
        assert_eq!(
            reason(&evaluate(
                &e,
                Some((&r, LeaderStatus::Active)),
                &cfg,
                &gated
            )),
            Some(RejectReason::SymbolGated)
        );
        let mut skip_sniper = cfg.clone();
        skip_sniper.skip_if_sniper_holds = true;
        let sniper = PolicyContext {
            sniper_holds: true,
            ..ctx
        };
        assert_eq!(
            reason(&evaluate(
                &e,
                Some((&r, LeaderStatus::Active)),
                &skip_sniper,
                &sniper
            )),
            Some(RejectReason::SniperHolds)
        );
        assert!(matches!(
            evaluate(&e, Some((&r, LeaderStatus::Active)), &cfg, &sniper),
            PolicyVerdict::Enter(_)
        ));
        let holding = PolicyContext {
            holding: true,
            held_qty: 10.0,
            ..ctx
        };
        assert_eq!(
            reason(&evaluate(
                &e,
                Some((&r, LeaderStatus::Active)),
                &cfg,
                &holding
            )),
            Some(RejectReason::AlreadyMirroring)
        );
        match evaluate(&e, Some((&r, LeaderStatus::Active)), &cfg, &ctx) {
            PolicyVerdict::Enter(plan) => {
                assert_eq!(plan.slippage_pct, cfg.slippage_pct);
                assert!(plan.observed_age_secs <= 1);
                assert!(plan.event_age_secs >= 1);
            }
            other => panic!("expected Enter, got {other:?}"),
        }
        let mut custom = r.clone();
        custom.slippage_pct = Some(7.5);
        match evaluate(&e, Some((&custom, LeaderStatus::Active)), &cfg, &ctx) {
            PolicyVerdict::Enter(plan) => assert_eq!(plan.slippage_pct, 7.5),
            other => panic!("expected Enter, got {other:?}"),
        }
    }

    #[test]
    fn sell_rules_and_exit_fraction() {
        let cfg = CopyConfig::default();
        let e = event(PositionSide::Short, 1.0);
        let r = rule();
        let none = PolicyContext::clean(Utc::now());
        let holding = PolicyContext {
            holding: true,
            held_qty: 400.0,
            ..none
        };
        assert_eq!(
            reason(&evaluate(&e, None, &cfg, &holding)),
            Some(RejectReason::LeaderUnknown)
        );
        assert_eq!(
            reason(&evaluate(
                &e,
                Some((&r, LeaderStatus::Removed)),
                &cfg,
                &holding
            )),
            Some(RejectReason::LeaderRemoved)
        );
        let mut off = cfg.clone();
        off.mirror_exits = false;
        assert_eq!(
            reason(&evaluate(
                &e,
                Some((&r, LeaderStatus::Active)),
                &off,
                &holding
            )),
            Some(RejectReason::MirrorExitsDisabled)
        );
        let mut buys_only = r.clone();
        buys_only.buys_only = true;
        assert_eq!(
            reason(&evaluate(
                &e,
                Some((&buys_only, LeaderStatus::Active)),
                &cfg,
                &holding
            )),
            Some(RejectReason::SellsNotMirrored)
        );
        assert_eq!(
            reason(&evaluate(&e, Some((&r, LeaderStatus::Active)), &cfg, &none)),
            Some(RejectReason::NoPositionToExit)
        );
        // Paused leaders still get their exits mirrored.
        match evaluate(&e, Some((&r, LeaderStatus::Paused)), &cfg, &holding) {
            PolicyVerdict::Exit(plan) => assert_eq!(plan.fraction, 1.0),
            other => panic!("expected Exit, got {other:?}"),
        }
        let mut partial = cfg.clone();
        partial.full_exit_on_their_exit = false;
        match evaluate(&e, Some((&r, LeaderStatus::Active)), &partial, &holding) {
            PolicyVerdict::Exit(plan) => assert!((plan.fraction - 0.25).abs() < 1e-9),
            other => panic!("expected Exit, got {other:?}"),
        }
        let big = event(PositionSide::Short, 1.0);
        let tiny_holding = PolicyContext {
            held_qty: 10.0,
            ..holding
        };
        match evaluate(
            &big,
            Some((&r, LeaderStatus::Active)),
            &partial,
            &tiny_holding,
        ) {
            PolicyVerdict::Exit(plan) => assert_eq!(plan.fraction, 1.0, "clamped to full close"),
            other => panic!("expected Exit, got {other:?}"),
        }
        // Sells ignore min_sol, staleness and venue decoders.
        let mut stale_small = event(PositionSide::Short, 0.0001);
        stale_small.observed_at = Utc::now() - Duration::seconds(600);
        stale_small.block_time = Some(Utc::now() - Duration::seconds(600));
        let mut no_pump = cfg.clone();
        no_pump.decode_pumpfun = false;
        assert!(matches!(
            evaluate(
                &stale_small,
                Some((&r, LeaderStatus::Active)),
                &no_pump,
                &holding
            ),
            PolicyVerdict::Exit(_)
        ));
    }

    #[test]
    fn venue_flags_map_to_decoders() {
        let mut cfg = CopyConfig::default();
        assert!(venue_enabled(&cfg, Venue::PumpFun));
        assert!(venue_enabled(&cfg, Venue::RaydiumClmm));
        assert!(venue_enabled(&cfg, Venue::Paper));
        assert!(!venue_enabled(&cfg, Venue::PolymarketClob));
        cfg.decode_raydium = false;
        assert!(!venue_enabled(&cfg, Venue::RaydiumAmmV4));
        assert!(!venue_enabled(&cfg, Venue::RaydiumClmm));
        cfg.decode_jupiter = false;
        assert!(!venue_enabled(&cfg, Venue::Jupiter));
        cfg.decode_pumpswap = false;
        assert!(!venue_enabled(&cfg, Venue::PumpSwap));
    }
}
