//! Global risk audit trail (TASK 5 §9).
//!
//! Published as [`AppEvent::Audit`] with `actor = "global-risk"`
//! ([`AUDIT_ACTOR`]):
//!
//! | action | when |
//! |---|---|
//! | `global.risk.reject` | a request was refused (target = decision id; outcome = the full decision line) |
//! | `global.kill_switch.<engage\|release\|restored>` | a venue / strategy switch changed (target = scope) |
//!
//! Accepted decisions are journaled (`global_risk_decisions`) and metered
//! but not audited one by one — the audit bus is for consequential
//! mutations, and an accept mutates nothing.

use chrono::Utc;

use super::decision::GlobalRiskDecision;
use crate::events::{AppEvent, EventBus};

/// Actor recorded on every global-risk audit event.
pub const AUDIT_ACTOR: &str = "global-risk";

fn publish(bus: &EventBus, action: String, target: String, outcome: String) {
    bus.publish(AppEvent::Audit {
        ts: Utc::now(),
        actor: AUDIT_ACTOR.into(),
        action,
        target: Some(target),
        outcome: crate::accounting::audit::sanitize(&outcome),
    });
}

/// One rejection.
pub(crate) fn decision(bus: &EventBus, d: &GlobalRiskDecision) {
    publish(
        bus,
        "global.risk.reject".into(),
        d.decision_id.clone(),
        d.summary(),
    );
}

/// One kill-switch change.
pub(crate) fn kill_switch(bus: &EventBus, action: &str, target: &str, detail: &str) {
    publish(
        bus,
        format!("global.kill_switch.{action}"),
        target.to_string(),
        detail.to_string(),
    );
}
