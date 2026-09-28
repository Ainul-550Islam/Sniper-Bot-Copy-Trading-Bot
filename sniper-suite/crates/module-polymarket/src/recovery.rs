//! Restart recovery — deterministic re-adoption of what the journals say.
//!
//! [`PolyBot::recover_after_restart`] runs before the first scan. It re-adopts
//! every non-terminal venue order from the durable journal (`poly_orders`)
//! and every incomplete Polymarket OMS order that carries a venue id, holds
//! ambiguous ones (`submitted` / `unknown`) for reconciliation, fails paper
//! orders that never reached their in-process fill and OMS orders that never
//! reached the venue, then (live, authenticated) reconciles immediately.
//! Nothing is ever re-submitted: a fill or a cancel comes only from the venue.
//! Every action is metered (`poly_recovery_actions_total{action}`) and
//! audited (`poly.recovery.<action>`).

use chrono::Utc;
use tracing::warn;

use bot_core::models::{BotModule, ExecutionMode};

use crate::error::PolyResult;
use crate::metrics;
use crate::orders::{LocalOrderState, TrackedOrder};
use crate::reconcile::ReconFinding;
use crate::PolyBot;

/// Restart-recovery actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecoveryAction {
    /// A non-terminal venue order from the journal is tracked again.
    AdoptedJournalOrder,
    /// An incomplete OMS order with a venue id (but no journal row) is
    /// tracked again in `Unknown` state.
    AdoptedOmsOrder,
    /// A submit-unknown / unknown order is held for reconciliation.
    HeldAmbiguous,
    /// A paper order that never reached its (in-process) fill is failed.
    FailedStalePaper,
    /// An incomplete OMS order that can never be on the venue — no venue id
    /// was ever attached (the pipeline attaches it before the POST), or it
    /// is a paper/simulate order whose fill is in-process — is failed so it
    /// does not come back as `Unknown` on every restart (the OMS reloads
    /// only non-terminal orders) and stops counting as open.
    FailedUnsent,
}

impl RecoveryAction {
    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            RecoveryAction::AdoptedJournalOrder => "adopted_journal_order",
            RecoveryAction::AdoptedOmsOrder => "adopted_oms_order",
            RecoveryAction::HeldAmbiguous => "held_ambiguous",
            RecoveryAction::FailedStalePaper => "failed_stale_paper",
            RecoveryAction::FailedUnsent => "failed_unsent",
        }
    }
}

/// Summary of one restart recovery pass.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecoveryReport {
    /// `(action, venue_order_id)` in the order they were taken.
    pub actions: Vec<(RecoveryAction, String)>,
    /// Findings produced by the reconciliation run at the end (live only).
    pub findings: Vec<ReconFinding>,
}

impl RecoveryReport {
    /// Count of one action kind.
    pub fn count(&self, action: RecoveryAction) -> usize {
        self.actions.iter().filter(|(a, _)| *a == action).count()
    }
}

impl PolyBot {
    /// Re-adopt every non-terminal venue order from the journal and every
    /// incomplete Polymarket OMS order that carries a venue id, hold
    /// ambiguous ones for reconciliation, fail paper orders that never
    /// reached their in-process fill, then (live) reconcile immediately.
    pub async fn recover_after_restart(&self) -> PolyResult<RecoveryReport> {
        let mut report = RecoveryReport::default();
        let now = Utc::now();
        let cfg = self.state.config_snapshot().await;

        let journaled = match self.store.open_orders().await {
            Some(v) => v,
            None => {
                metrics::count_journal_error("open_orders");
                Vec::new()
            }
        };
        for rec in journaled {
            if self.get_tracked(&rec.venue_order_id).await.is_some() {
                continue;
            }
            let mode = rec
                .mode
                .parse::<ExecutionMode>()
                .unwrap_or(ExecutionMode::Paper);
            let state = LocalOrderState::parse(&rec.state);
            let question = self
                .markets
                .read()
                .await
                .get(&rec.condition_id)
                .map(|m| m.question.clone())
                .unwrap_or_default();
            let mut tracked = TrackedOrder {
                venue_order_id: rec.venue_order_id.clone(),
                order_id: rec.order_id.clone(),
                signal_id: rec.signal_id.clone(),
                condition_id: rec.condition_id.clone(),
                token_id: rec.token_id.clone(),
                outcome: rec.outcome.clone(),
                question,
                is_buy: rec.side == "buy",
                neg_risk: false,
                order_type: rec.order_type.clone(),
                limit_price: rec.limit_price,
                size_tokens: rec.size_tokens,
                size_matched: rec.size_matched,
                mode,
                state,
                venue_status: rec.venue_status.clone(),
                expiration: rec.expiration.max(0) as u64,
                position_id: rec.position_id.clone(),
                signature: None,
                submitted_at: rec.submitted_at,
                updated_at: now,
                booked_trade_ids: Vec::new(),
                trade_matched: 0.0,
            };
            if let Some(o) = self.orders.get(&rec.order_id).await {
                tracked.signature = o.signature.clone();
            }
            if mode != ExecutionMode::Live {
                // Paper orders fill inside the same call that submits them;
                // an open paper row means the process died in between.
                self.finish_locally(
                    &mut tracked,
                    LocalOrderState::Failed,
                    "stale paper order after restart",
                )
                .await;
                report
                    .actions
                    .push((RecoveryAction::FailedStalePaper, rec.venue_order_id.clone()));
                continue;
            }
            self.insert_tracked(tracked.clone()).await;
            report.actions.push((
                RecoveryAction::AdoptedJournalOrder,
                rec.venue_order_id.clone(),
            ));
            if matches!(state, LocalOrderState::Submitted | LocalOrderState::Unknown) {
                report
                    .actions
                    .push((RecoveryAction::HeldAmbiguous, rec.venue_order_id.clone()));
            }
        }

        // OMS orders without a journal row (older releases, persistence-
        // layer created rows): adopt as Unknown when they carry a live venue
        // id; fail the ones that can never be on the venue.
        for o in self.orders.incomplete().await {
            if o.module != BotModule::Polymarket {
                continue;
            }
            let external = o.external_id.clone().filter(|s| !s.trim().is_empty());
            let never_sent = match &external {
                // The venue id is attached at SIGNED, before the POST: no
                // id means the order was never signed, let alone sent.
                None => true,
                // Paper/simulate fills happen inside the submitting call.
                Some(e) => e.starts_with("paper:") || o.mode != ExecutionMode::Live,
            };
            if never_sent {
                self.fail_oms(&o.id, "restart recovery: order never reached the venue")
                    .await;
                report.actions.push((
                    RecoveryAction::FailedUnsent,
                    external.unwrap_or(o.id.clone()),
                ));
                continue;
            }
            let Some(external) = external else {
                continue;
            };
            if self.get_tracked(&external).await.is_some() {
                continue;
            }
            let meta = &o.meta;
            let condition_id = meta
                .get("condition_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let tracked = TrackedOrder {
                venue_order_id: external.clone(),
                order_id: o.id.clone(),
                signal_id: meta
                    .get("signal_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                condition_id,
                token_id: o.symbol.clone(),
                outcome: meta
                    .get("outcome")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                question: meta
                    .get("question")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                is_buy: o.side != "sell",
                neg_risk: meta
                    .get("neg_risk")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                order_type: meta
                    .get("order_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("GTC")
                    .to_string(),
                limit_price: o.price.unwrap_or(0.0),
                size_tokens: o.qty,
                size_matched: 0.0,
                mode: o.mode,
                state: LocalOrderState::Unknown,
                venue_status: String::new(),
                expiration: meta.get("expiration").and_then(|v| v.as_u64()).unwrap_or(0),
                position_id: None,
                signature: o.signature.clone(),
                submitted_at: o.created_at,
                updated_at: now,
                booked_trade_ids: Vec::new(),
                trade_matched: 0.0,
            };
            self.insert_tracked(tracked.clone()).await;
            self.journal_order(&tracked).await;
            report
                .actions
                .push((RecoveryAction::AdoptedOmsOrder, external.clone()));
            report
                .actions
                .push((RecoveryAction::HeldAmbiguous, external));
        }

        for (action, id) in &report.actions {
            metrics::count_recovery_action(action.as_str());
            self.audit(
                &format!("poly.recovery.{}", action.as_str()),
                id,
                action.as_str(),
            );
        }

        if self.state.execution_mode().await == ExecutionMode::Live
            && self.api_key.read().await.is_some()
            && !report.actions.is_empty()
        {
            match self.reconcile_once(&cfg.polymarket).await {
                Ok(f) => report.findings = f,
                Err(e) => warn!(error = %e, "post-recovery reconciliation failed"),
            }
        }
        Ok(report)
    }
}
