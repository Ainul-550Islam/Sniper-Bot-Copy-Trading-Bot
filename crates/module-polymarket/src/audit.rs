//! Polymarket audit trail.
//!
//! Every consequential decision the engine takes is published as an
//! [`AppEvent::Audit`] on the shared event bus — the same bus the server
//! journals to disk / Postgres and the dashboard tails — with
//! `actor = "polymarket"` ([`AUDIT_ACTOR`]) and a dotted action:
//!
//! | action | when |
//! |---|---|
//! | `poly.signal.<stage>` | a signal reached its terminal pipeline stage (`resting`, `filled`, `rejected`, `failed`, `ambiguous`) — `pipeline.rs` |
//! | `poly.order.<state>` | a tracked venue order reached a terminal state (`filled`, `cancelled`, `expired`, `failed`) — `lifecycle.rs` |
//! | `poly.recon.<kind>` | a reconciliation finding — `reconcile.rs` |
//! | `poly.recovery.<action>` | a restart-recovery action — `recovery.rs` |
//!
//! The `outcome` string is a flat `key=value` line (token, price, size,
//! stake, order / venue / position ids, stage or reason, detail): greppable
//! in the JSONL journal and stable enough for alerting. Nothing here is a
//! source of truth — the durable journal (`poly_signals`, `poly_orders`,
//! `poly_fills`, `poly_recon_findings`) is.

use chrono::Utc;

use bot_core::events::AppEvent;

use crate::PolyBot;

/// Actor recorded on every audit event this module publishes.
pub const AUDIT_ACTOR: &str = "polymarket";

impl PolyBot {
    /// Publish one audit record (`actor = polymarket`) on the shared event
    /// bus; `outcome` is a flat single-line `key=value` text.
    pub(crate) fn audit(&self, action: &str, target: &str, outcome: &str) {
        self.state.events.publish(AppEvent::Audit {
            ts: Utc::now(),
            actor: AUDIT_ACTOR.into(),
            action: action.to_string(),
            target: Some(target.to_string()),
            outcome: outcome.to_string(),
        });
    }
}

/// Keep audit text single-line and bounded.
pub(crate) fn sanitize(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if out.len() > 240 {
        out.truncate(240);
        out.push('…');
    }
    out
}
