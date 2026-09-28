//! Global accounting audit trail (TASK 5 §9).
//!
//! Every ledger mutation, position mutation, reconciliation finding and
//! recovery action is published as an [`AppEvent::Audit`] on the shared
//! event bus — the same bus the server journals to disk / the hash-chained
//! `audit_events` table and the dashboard tails — with
//! `actor = "global-ledger"` ([`AUDIT_ACTOR`]) and a dotted action that
//! makes the mutation class greppable:
//!
//! | action | when |
//! |---|---|
//! | `global.ledger.<kind>` | an event was booked (`fill`, `fee`, `settlement`, `deposit`, …) |
//! | `global.position.<transition>` | the aggregated position `opened` / `increased` / `reduced` / `closed` |
//! | `global.recon.<kind>` | a new accounting reconciliation finding |
//! | `global.recovery.<action>` | a restart-recovery action |
//!
//! The `outcome` text is a flat `key=value` line. Nothing here is a source
//! of truth — the journal (`ledger_events`, `ledger_postings`) is.

use chrono::Utc;

use super::book::BookEffect;
use super::event::AccountingEvent;
use super::reconcile::AccountingFinding;
use crate::events::{AppEvent, EventBus};

/// Actor recorded on every accounting audit event.
pub const AUDIT_ACTOR: &str = "global-ledger";

fn publish(bus: &EventBus, action: String, target: String, outcome: String) {
    bus.publish(AppEvent::Audit {
        ts: Utc::now(),
        actor: AUDIT_ACTOR.into(),
        action,
        target: Some(target),
        outcome: sanitize(&outcome),
    });
}

/// One booked event + the position transition it caused.
pub(crate) fn ledger_mutation(
    bus: &EventBus,
    event: &AccountingEvent,
    event_id: &str,
    effect: &BookEffect,
) {
    publish(
        bus,
        format!("global.ledger.{}", event.kind),
        event_id.to_string(),
        format!("event={} {}", event_id, event.summary()),
    );
    if effect.transition != "cash" {
        publish(
            bus,
            format!("global.position.{}", effect.transition),
            effect.key.as_string(),
            format!(
                "event={} qty_before={:.8} qty_after={:.8} realized_delta={:.8} fee={:.8} cost_of_slice={:.8}",
                event_id,
                effect.qty_before,
                effect.qty_after,
                effect.realized_delta,
                effect.fee_delta,
                effect.cost_of_slice
            ),
        );
    }
}

/// One new reconciliation finding.
pub(crate) fn finding(bus: &EventBus, f: &AccountingFinding) {
    publish(
        bus,
        format!("global.recon.{}", f.kind),
        f.finding_id.clone(),
        f.summary(),
    );
}

/// One recovery action.
pub(crate) fn recovery(bus: &EventBus, action: &str, detail: &str) {
    publish(
        bus,
        format!("global.recovery.{action}"),
        "ledger".into(),
        detail.to_string(),
    );
}

/// Keep audit text single-line and bounded.
pub(crate) fn sanitize(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if out.len() > 480 {
        let mut cut = 480;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_is_single_line_and_bounded() {
        assert_eq!(sanitize("a\nb\tc"), "a b c");
        let long = "é".repeat(600);
        let s = sanitize(&long);
        assert!(s.chars().count() <= 481);
        assert!(s.ends_with('…'));
    }
}
