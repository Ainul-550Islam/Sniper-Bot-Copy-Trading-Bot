//! Copy-engine audit trail (TASK 3 §11).
//!
//! Every consequential decision the engine takes is published as an
//! [`AppEvent::Audit`] on the shared event bus — the same bus the server
//! journals to disk / Postgres and the dashboard tails — with
//! `actor = "copy"` and a dotted action:
//!
//! | action | when |
//! |---|---|
//! | `copy.entry.<stage>` | an entry event reached a terminal stage (`filled`, `rejected`, `failed`, `ambiguous`) |
//! | `copy.exit.<stage>` | a leader exit was mirrored / refused |
//! | `copy.leader.<event>` | registry transition (`followed`, `paused`, `resumed`, `unfollowed`, `rule_changed`) |
//! | `copy.recon.<kind>` | a reconciliation finding |
//! | `copy.recovery.<action>` | a restart-recovery action |
//!
//! The `outcome` string is a flat `key=value` line: greppable in the JSONL
//! journal, and stable enough for alerting. Nothing here is a source of
//! truth — the durable journal (`copy_events`, `copy_links`) is.

use bot_core::events::AppEvent;
use bot_core::state::Shared;
use chrono::Utc;

use crate::event::{CopyOutcome, LeaderTradeEvent};

/// The actor every copy audit record carries.
pub const AUDIT_ACTOR: &str = "copy";

/// Publish the terminal outcome of one entry (leader buy) event.
pub fn publish_entry(state: &Shared, event: &LeaderTradeEvent, outcome: &CopyOutcome) {
    publish_outcome(state, "entry", event, outcome);
}

/// Publish the terminal outcome of one exit (leader sell) event.
pub fn publish_exit(state: &Shared, event: &LeaderTradeEvent, outcome: &CopyOutcome) {
    publish_outcome(state, "exit", event, outcome);
}

fn publish_outcome(state: &Shared, kind: &str, event: &LeaderTradeEvent, outcome: &CopyOutcome) {
    let text = match &outcome.rejection {
        Some(r) => format!(
            "{} reason={} at={} detail={} leader={} sig={} slot={} source={} sol={:.6} total_ms={}",
            outcome.stage,
            r.reason,
            r.stage,
            sanitize(&r.detail),
            event.leader,
            event.signature,
            event.slot,
            event.source.as_str(),
            event.sol_amount,
            outcome.total_ms
        ),
        None => format!(
            "{} leader={} sig={} slot={} source={} leader_sol={:.6} requested_sol={} sized_sol={} intent={} position={} signature={} total_ms={}",
            outcome.stage,
            event.leader,
            event.signature,
            event.slot,
            event.source.as_str(),
            event.sol_amount,
            fmt_opt_f64(outcome.requested_sol),
            fmt_opt_f64(outcome.sized_sol),
            outcome.intent_id.as_deref().unwrap_or("-"),
            outcome.position_id.as_deref().unwrap_or("-"),
            outcome.signature.as_deref().unwrap_or("-"),
            outcome.total_ms
        ),
    };
    state.events.publish(AppEvent::Audit {
        ts: Utc::now(),
        actor: AUDIT_ACTOR.into(),
        action: format!(
            "copy.{kind}.{}",
            outcome.stage.as_str().to_ascii_lowercase()
        ),
        target: Some(format!("{}:{}", event.mint, event.event_id)),
        outcome: text,
    });
}

/// Publish a leader registry transition.
pub fn publish_leader(state: &Shared, address: &str, event: &str, reason: Option<&str>) {
    state.events.publish(AppEvent::Audit {
        ts: Utc::now(),
        actor: AUDIT_ACTOR.into(),
        action: format!("copy.leader.{event}"),
        target: Some(address.to_string()),
        outcome: format!(
            "{event} leader={address} reason={}",
            sanitize(reason.unwrap_or("-"))
        ),
    });
}

/// Publish a reconciliation finding.
pub fn publish_recon(state: &Shared, kind: &str, leader: &str, mint: &str, detail: &str) {
    state.events.publish(AppEvent::Audit {
        ts: Utc::now(),
        actor: AUDIT_ACTOR.into(),
        action: format!("copy.recon.{kind}"),
        target: Some(format!("{leader}:{mint}")),
        outcome: format!(
            "{kind} leader={leader} mint={mint} detail={}",
            sanitize(detail)
        ),
    });
}

/// Publish a restart-recovery action.
pub fn publish_recovery(state: &Shared, action: &str, target: &str, detail: &str) {
    state.events.publish(AppEvent::Audit {
        ts: Utc::now(),
        actor: AUDIT_ACTOR.into(),
        action: format!("copy.recovery.{action}"),
        target: Some(target.to_string()),
        outcome: format!("{action} target={target} detail={}", sanitize(detail)),
    });
}

/// Keep the `key=value` line parseable: collapse whitespace/newlines.
fn sanitize(s: &str) -> String {
    let collapsed: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        "-".into()
    } else {
        collapsed
    }
}

fn fmt_opt_f64(v: Option<f64>) -> String {
    match v {
        Some(x) => format!("{x:.6}"),
        None => "-".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_collapses_whitespace_and_empty() {
        assert_eq!(sanitize("a  b\nc\t d"), "a b c d");
        assert_eq!(sanitize("   "), "-");
        assert_eq!(fmt_opt_f64(None), "-");
        assert_eq!(fmt_opt_f64(Some(0.5)), "0.500000");
    }
}
