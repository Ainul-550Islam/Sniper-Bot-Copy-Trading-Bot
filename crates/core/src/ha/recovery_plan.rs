//! The deterministic order/execution recovery matrix (TASK 6 §5, §6).
//!
//! On restart or after a takeover, every order the suite knows about must
//! resolve to **exactly one** recovery action. This module is PURE: it maps
//! an observed situation onto an action, with no I/O, so the whole matrix is
//! unit-testable and the adapters (`server/src/ha.rs`, the module recovery
//! paths) stay thin.
//!
//! # The eight order situations (§6)
//!
//! | # | Situation | Evidence | Action |
//! |---|---|---|---|
//! | A | never submitted | OMS `Created`/`Validated`, no signature, no venue id | `CloseUnsent` — nothing left the process |
//! | B | journaled, not submitted | write-ahead row exists, execution ledger has no `Submitted` | `CloseUnsent` |
//! | C | submitted, response unknown | `Submitted`/`Pending`, or `Unknown` with a signature | `HoldAmbiguous` + `EnqueueReconcile` — never resubmit |
//! | D | venue acknowledged, local state missing | venue/chain shows the order, local row absent or non-terminal | `AdoptFromVenue` |
//! | E | partially filled | venue reports `matched < size` | `ResumeTracking` (keep the booked part, keep polling) |
//! | F | fully filled | venue reports `matched == size` | `FinalizeFilled` (book what is missing, idempotently) |
//! | G | cancelled | venue confirms cancelled | `FinalizeCancelled` |
//! | H | expired | venue confirms expired / TTL passed with no fill | `FinalizeExpired` |
//!
//! Two rules make this safe:
//!
//! * **Never blindly resubmit.** There is no `Resubmit` action. An
//!   ambiguous send is held and handed to reconciliation; only a venue read
//!   can turn it into a finalize action.
//! * **Unknown evidence is not "nothing happened".** When the venue could
//!   not be read, the answer is `HoldAmbiguous`, not a finalize.

use serde::{Deserialize, Serialize};

/// What the local records say about an order at recovery time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalOrderEvidence {
    /// No local order row at all (only a venue observation).
    Absent,
    /// Created / validated: the write-ahead journal has it, nothing was
    /// sent (the `Submitted` row is written BEFORE the send, so its absence
    /// proves nothing left the process).
    JournaledNotSent,
    /// A send was started: the outcome may or may not have happened.
    SubmittedUnknown,
    /// Local state says the order rests on the venue.
    Open,
    /// Local state is already terminal.
    Terminal,
}

impl LocalOrderEvidence {
    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            LocalOrderEvidence::Absent => "absent",
            LocalOrderEvidence::JournaledNotSent => "journaled_not_sent",
            LocalOrderEvidence::SubmittedUnknown => "submitted_unknown",
            LocalOrderEvidence::Open => "open",
            LocalOrderEvidence::Terminal => "terminal",
        }
    }
}

/// What the venue says about the same order at recovery time.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VenueOrderEvidence {
    /// The venue was not readable (network, no credentials, no RPC). NOT a
    /// statement that the order does not exist.
    Unavailable,
    /// The venue answered and knows nothing about it.
    Absent,
    /// Resting, nothing matched.
    Open,
    /// Partially matched.
    PartiallyFilled {
        /// Matched base quantity the venue reports.
        matched: f64,
        /// Total order size.
        size: f64,
    },
    /// Fully matched.
    Filled {
        /// Matched base quantity the venue reports.
        matched: f64,
    },
    /// Cancelled on the venue.
    Cancelled,
    /// Expired on the venue.
    Expired,
}

impl VenueOrderEvidence {
    /// Stable label (metrics / journal).
    pub fn as_str(&self) -> &'static str {
        match self {
            VenueOrderEvidence::Unavailable => "unavailable",
            VenueOrderEvidence::Absent => "absent",
            VenueOrderEvidence::Open => "open",
            VenueOrderEvidence::PartiallyFilled { .. } => "partially_filled",
            VenueOrderEvidence::Filled { .. } => "filled",
            VenueOrderEvidence::Cancelled => "cancelled",
            VenueOrderEvidence::Expired => "expired",
        }
    }
}

/// The deterministic recovery actions (§6). Closed vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderRecoveryAction {
    /// Nothing was broadcast: close the intent locally as failed-unsent.
    CloseUnsent,
    /// The send may have landed: hold it, let reconciliation decide. No
    /// resubmission, no fabricated fill.
    HoldAmbiguous,
    /// The venue knows an order we do not: adopt it into local tracking.
    AdoptFromVenue,
    /// Still live with a partial fill: resume tracking (poll + user feed).
    ResumeTracking,
    /// Fully filled: book whatever the ledger is missing, idempotently.
    FinalizeFilled,
    /// Cancelled on the venue: close locally.
    FinalizeCancelled,
    /// Expired on the venue: close locally.
    FinalizeExpired,
    /// Local and venue agree and the state is already terminal: nothing.
    NoAction,
}

impl OrderRecoveryAction {
    /// Every action, stable order.
    pub const ALL: [OrderRecoveryAction; 8] = [
        OrderRecoveryAction::CloseUnsent,
        OrderRecoveryAction::HoldAmbiguous,
        OrderRecoveryAction::AdoptFromVenue,
        OrderRecoveryAction::ResumeTracking,
        OrderRecoveryAction::FinalizeFilled,
        OrderRecoveryAction::FinalizeCancelled,
        OrderRecoveryAction::FinalizeExpired,
        OrderRecoveryAction::NoAction,
    ];

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            OrderRecoveryAction::CloseUnsent => "close_unsent",
            OrderRecoveryAction::HoldAmbiguous => "hold_ambiguous",
            OrderRecoveryAction::AdoptFromVenue => "adopt_from_venue",
            OrderRecoveryAction::ResumeTracking => "resume_tracking",
            OrderRecoveryAction::FinalizeFilled => "finalize_filled",
            OrderRecoveryAction::FinalizeCancelled => "finalize_cancelled",
            OrderRecoveryAction::FinalizeExpired => "finalize_expired",
            OrderRecoveryAction::NoAction => "no_action",
        }
    }

    /// Inverse of [`OrderRecoveryAction::as_str`].
    pub fn parse(s: &str) -> Option<OrderRecoveryAction> {
        OrderRecoveryAction::ALL
            .iter()
            .copied()
            .find(|a| a.as_str() == s)
    }

    /// Does this action need a reconciliation claim to be enqueued?
    pub fn needs_reconcile(&self) -> bool {
        matches!(
            self,
            OrderRecoveryAction::HoldAmbiguous | OrderRecoveryAction::AdoptFromVenue
        )
    }

    /// Does this action book money (and therefore go through the ledger's
    /// idempotency)?
    pub fn books_money(&self) -> bool {
        matches!(
            self,
            OrderRecoveryAction::FinalizeFilled | OrderRecoveryAction::ResumeTracking
        )
    }
}

impl std::fmt::Display for OrderRecoveryAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One resolved order: the action plus the reason it was chosen.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OrderRecoveryPlan {
    /// The single action to take.
    pub action: OrderRecoveryAction,
    /// Human, deterministic explanation (goes into the audit record).
    pub reason: String,
    /// Local evidence the decision used.
    pub local: LocalOrderEvidence,
    /// Venue evidence the decision used.
    pub venue: VenueOrderEvidence,
}

impl OrderRecoveryPlan {
    /// Single-line audit text.
    pub fn summary(&self) -> String {
        format!(
            "action={} local={} venue={} reason={}",
            self.action,
            self.local.as_str(),
            self.venue.as_str(),
            self.reason
        )
    }
}

/// Resolve one order to exactly one action (the §6 matrix).
///
/// Deterministic and total: every `(local, venue)` pair has exactly one
/// answer, and the same pair always produces the same action.
pub fn plan_order_recovery(
    local: LocalOrderEvidence,
    venue: VenueOrderEvidence,
) -> OrderRecoveryPlan {
    use LocalOrderEvidence as L;
    use OrderRecoveryAction as A;
    use VenueOrderEvidence as V;

    let (action, reason): (A, String) = match (local, venue) {
        // --- The venue could not be read -------------------------------
        // Nothing was sent: safe to close regardless of the venue.
        (L::JournaledNotSent, V::Unavailable) => (
            A::CloseUnsent,
            "journaled but never submitted; the submit row is written before the send, so nothing was broadcast".into(),
        ),
        (L::Terminal, V::Unavailable) => (
            A::NoAction,
            "local state is already terminal; the venue read can wait".into(),
        ),
        (L::Absent, V::Unavailable) => (
            A::NoAction,
            "no local record and the venue could not be read; nothing to act on".into(),
        ),
        (_, V::Unavailable) => (
            A::HoldAmbiguous,
            "venue unreadable: an unavailable read is never evidence of absence".into(),
        ),

        // --- Case A/B: nothing was sent --------------------------------
        (L::JournaledNotSent, V::Absent) => (
            A::CloseUnsent,
            "journaled, not submitted, unknown to the venue: nothing was broadcast".into(),
        ),
        // A pre-send crash whose order nevertheless exists on the venue is
        // real (the send landed before the row was written): adopt it.
        (L::JournaledNotSent, V::Open) => (
            A::AdoptFromVenue,
            "journaled as unsent but resting on the venue: adopt the venue truth".into(),
        ),
        (L::JournaledNotSent, V::PartiallyFilled { matched, size }) => (
            A::AdoptFromVenue,
            format!("journaled as unsent but partially filled on the venue ({matched} of {size}): adopt the venue truth"),
        ),
        (L::JournaledNotSent, V::Filled { matched }) => (
            A::FinalizeFilled,
            format!("journaled as unsent but filled on the venue ({matched}): book the fill idempotently"),
        ),
        (L::JournaledNotSent, V::Cancelled) => (
            A::FinalizeCancelled,
            "journaled as unsent and cancelled on the venue".into(),
        ),
        (L::JournaledNotSent, V::Expired) => (
            A::FinalizeExpired,
            "journaled as unsent and expired on the venue".into(),
        ),

        // --- Case C: submitted, outcome unknown ------------------------
        (L::SubmittedUnknown, V::Absent) => (
            A::HoldAmbiguous,
            "submitted with an unknown outcome and absent from the venue: may still land, never resubmit".into(),
        ),
        (L::SubmittedUnknown, V::Open) => (
            A::ResumeTracking,
            "submitted with an unknown outcome and resting on the venue: resume tracking".into(),
        ),
        (L::SubmittedUnknown, V::PartiallyFilled { matched, size }) => (
            A::ResumeTracking,
            format!("submitted with an unknown outcome, partially filled ({matched} of {size}): resume tracking"),
        ),
        (L::SubmittedUnknown, V::Filled { matched }) => (
            A::FinalizeFilled,
            format!("submitted with an unknown outcome, filled on the venue ({matched})"),
        ),
        (L::SubmittedUnknown, V::Cancelled) => (
            A::FinalizeCancelled,
            "submitted with an unknown outcome, cancelled on the venue".into(),
        ),
        (L::SubmittedUnknown, V::Expired) => (
            A::FinalizeExpired,
            "submitted with an unknown outcome, expired on the venue".into(),
        ),

        // --- Case D: the venue knows an order we do not ----------------
        (L::Absent, V::Open) => (
            A::AdoptFromVenue,
            "resting on the venue with no local record: adopt it".into(),
        ),
        (L::Absent, V::PartiallyFilled { matched, size }) => (
            A::AdoptFromVenue,
            format!("partially filled on the venue ({matched} of {size}) with no local record: adopt it"),
        ),
        (L::Absent, V::Filled { matched }) => (
            A::FinalizeFilled,
            format!("filled on the venue ({matched}) with no local record: book the fill idempotently"),
        ),
        (L::Absent, V::Cancelled) => (
            A::NoAction,
            "cancelled on the venue and unknown locally: nothing to book".into(),
        ),
        (L::Absent, V::Expired) => (
            A::NoAction,
            "expired on the venue and unknown locally: nothing to book".into(),
        ),
        (L::Absent, V::Absent) => (
            A::NoAction,
            "unknown to both sides".into(),
        ),

        // --- Cases E/F/G/H against a live local order ------------------
        (L::Open, V::Open) => (
            A::ResumeTracking,
            "open on both sides: resume tracking".into(),
        ),
        (L::Open, V::PartiallyFilled { matched, size }) => (
            A::ResumeTracking,
            format!("partially filled on the venue ({matched} of {size}): keep the booked part and resume tracking"),
        ),
        (L::Open, V::Filled { matched }) => (
            A::FinalizeFilled,
            format!("filled on the venue ({matched}) while local state was open"),
        ),
        (L::Open, V::Cancelled) => (
            A::FinalizeCancelled,
            "cancelled on the venue while local state was open".into(),
        ),
        (L::Open, V::Expired) => (
            A::FinalizeExpired,
            "expired on the venue while local state was open".into(),
        ),
        (L::Open, V::Absent) => (
            A::HoldAmbiguous,
            "open locally but absent from the venue: acknowledged-then-vanished is ambiguous, not a failure".into(),
        ),

        // --- Local already terminal ------------------------------------
        (L::Terminal, V::Filled { matched }) => (
            A::FinalizeFilled,
            format!("local state terminal but the venue reports a fill ({matched}): book whatever is missing idempotently"),
        ),
        (L::Terminal, V::PartiallyFilled { matched, size }) => (
            A::FinalizeFilled,
            format!("local state terminal but the venue reports a partial fill ({matched} of {size}): book whatever is missing idempotently"),
        ),
        (L::Terminal, V::Open) => (
            A::AdoptFromVenue,
            "local state terminal but the order still rests on the venue: adopt and resolve".into(),
        ),
        (L::Terminal, V::Absent | V::Cancelled | V::Expired) => (
            A::NoAction,
            "local and venue agree the order is finished".into(),
        ),
    };

    OrderRecoveryPlan {
        action,
        reason,
        local,
        venue,
    }
}

/// The crash boundaries the suite is expected to survive (§5). Each maps to
/// the local evidence a restart will find, which is what makes the outcome
/// deterministic and testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrashBoundary {
    /// Crash before anything was persisted.
    BeforePersistence,
    /// Crash after the write-ahead journal, before the risk decision.
    AfterPersistence,
    /// Crash before the risk decision was taken.
    BeforeRiskDecision,
    /// Crash after the risk decision, before the order was built.
    AfterRiskDecision,
    /// Crash before the venue submission.
    BeforeSubmission,
    /// Crash after the submission call started.
    AfterSubmission,
    /// Crash before the venue acknowledgement was read.
    BeforeVenueAck,
    /// Crash after the venue acknowledged.
    AfterVenueAck,
    /// Crash before the fill was booked into the ledger.
    BeforeFillAccounting,
    /// Crash after the fill was booked.
    AfterFillAccounting,
    /// Crash while reconciliation was running.
    DuringReconciliation,
    /// Crash while recovery itself was running.
    DuringRecovery,
}

impl CrashBoundary {
    /// Every boundary, stable order.
    pub const ALL: [CrashBoundary; 12] = [
        CrashBoundary::BeforePersistence,
        CrashBoundary::AfterPersistence,
        CrashBoundary::BeforeRiskDecision,
        CrashBoundary::AfterRiskDecision,
        CrashBoundary::BeforeSubmission,
        CrashBoundary::AfterSubmission,
        CrashBoundary::BeforeVenueAck,
        CrashBoundary::AfterVenueAck,
        CrashBoundary::BeforeFillAccounting,
        CrashBoundary::AfterFillAccounting,
        CrashBoundary::DuringReconciliation,
        CrashBoundary::DuringRecovery,
    ];

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            CrashBoundary::BeforePersistence => "before_persistence",
            CrashBoundary::AfterPersistence => "after_persistence",
            CrashBoundary::BeforeRiskDecision => "before_risk_decision",
            CrashBoundary::AfterRiskDecision => "after_risk_decision",
            CrashBoundary::BeforeSubmission => "before_submission",
            CrashBoundary::AfterSubmission => "after_submission",
            CrashBoundary::BeforeVenueAck => "before_venue_ack",
            CrashBoundary::AfterVenueAck => "after_venue_ack",
            CrashBoundary::BeforeFillAccounting => "before_fill_accounting",
            CrashBoundary::AfterFillAccounting => "after_fill_accounting",
            CrashBoundary::DuringReconciliation => "during_reconciliation",
            CrashBoundary::DuringRecovery => "during_recovery",
        }
    }

    /// Inverse of [`CrashBoundary::as_str`].
    pub fn parse(s: &str) -> Option<CrashBoundary> {
        CrashBoundary::ALL.iter().copied().find(|b| b.as_str() == s)
    }

    /// What a restart finds locally after a crash at this boundary.
    pub fn local_evidence(&self) -> LocalOrderEvidence {
        match self {
            // Nothing durable was written yet.
            CrashBoundary::BeforePersistence | CrashBoundary::BeforeRiskDecision => {
                LocalOrderEvidence::Absent
            }
            // Journalled intent, nothing sent.
            CrashBoundary::AfterPersistence
            | CrashBoundary::AfterRiskDecision
            | CrashBoundary::BeforeSubmission => LocalOrderEvidence::JournaledNotSent,
            // A send was started; the outcome is unknown until the venue answers.
            CrashBoundary::AfterSubmission | CrashBoundary::BeforeVenueAck => {
                LocalOrderEvidence::SubmittedUnknown
            }
            // The venue acknowledged: the order is live locally.
            CrashBoundary::AfterVenueAck | CrashBoundary::BeforeFillAccounting => {
                LocalOrderEvidence::Open
            }
            // The fill was booked (ledger idempotency protects a repeat).
            CrashBoundary::AfterFillAccounting => LocalOrderEvidence::Terminal,
            // Both workers restart mid-sweep: the orders they were resolving
            // are whatever they were; the sweep itself simply runs again.
            CrashBoundary::DuringReconciliation | CrashBoundary::DuringRecovery => {
                LocalOrderEvidence::Open
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use LocalOrderEvidence as L;
    use OrderRecoveryAction as A;
    use VenueOrderEvidence as V;

    #[test]
    fn every_pair_resolves_to_exactly_one_action_deterministically() {
        let locals = [
            L::Absent,
            L::JournaledNotSent,
            L::SubmittedUnknown,
            L::Open,
            L::Terminal,
        ];
        let venues = [
            V::Unavailable,
            V::Absent,
            V::Open,
            V::PartiallyFilled {
                matched: 1.0,
                size: 2.0,
            },
            V::Filled { matched: 2.0 },
            V::Cancelled,
            V::Expired,
        ];
        for l in locals {
            for v in venues {
                let a = plan_order_recovery(l, v);
                let b = plan_order_recovery(l, v);
                assert_eq!(a, b, "the same inputs must give the same plan");
                assert!(!a.reason.is_empty());
                assert!(a.summary().contains(a.action.as_str()));
            }
        }
    }

    #[test]
    fn the_eight_situations_of_section_six() {
        // A: never submitted.
        assert_eq!(
            plan_order_recovery(L::Absent, V::Absent).action,
            A::NoAction
        );
        // B: journaled but not submitted.
        assert_eq!(
            plan_order_recovery(L::JournaledNotSent, V::Absent).action,
            A::CloseUnsent
        );
        // C: submitted, response unknown.
        let c = plan_order_recovery(L::SubmittedUnknown, V::Absent);
        assert_eq!(c.action, A::HoldAmbiguous);
        assert!(c.action.needs_reconcile());
        // D: venue acknowledged, local state missing.
        assert_eq!(
            plan_order_recovery(L::Absent, V::Open).action,
            A::AdoptFromVenue
        );
        // E: partially filled.
        assert_eq!(
            plan_order_recovery(
                L::Open,
                V::PartiallyFilled {
                    matched: 3.0,
                    size: 10.0
                }
            )
            .action,
            A::ResumeTracking
        );
        // F: fully filled.
        assert_eq!(
            plan_order_recovery(L::Open, V::Filled { matched: 10.0 }).action,
            A::FinalizeFilled
        );
        // G: cancelled.
        assert_eq!(
            plan_order_recovery(L::Open, V::Cancelled).action,
            A::FinalizeCancelled
        );
        // H: expired.
        assert_eq!(
            plan_order_recovery(L::Open, V::Expired).action,
            A::FinalizeExpired
        );
    }

    #[test]
    fn ambiguity_is_never_resolved_by_guessing() {
        // No action in the vocabulary resubmits.
        for a in A::ALL {
            assert!(
                !a.as_str().contains("resubmit"),
                "{a} must not be a resubmission"
            );
        }
        // An unreadable venue never finalizes a live order.
        for l in [L::SubmittedUnknown, L::Open] {
            assert_eq!(
                plan_order_recovery(l, V::Unavailable).action,
                A::HoldAmbiguous
            );
        }
        // Acknowledged-then-vanished is held, not failed.
        assert_eq!(
            plan_order_recovery(L::Open, V::Absent).action,
            A::HoldAmbiguous
        );
        // But a provably unsent order is closed even with an unreadable venue.
        assert_eq!(
            plan_order_recovery(L::JournaledNotSent, V::Unavailable).action,
            A::CloseUnsent
        );
    }

    #[test]
    fn crash_boundaries_map_to_deterministic_local_evidence() {
        use CrashBoundary as C;
        assert_eq!(C::ALL.len(), 12);
        for b in C::ALL {
            assert_eq!(CrashBoundary::parse(b.as_str()), Some(b));
            // Every boundary resolves to exactly one plan against a readable venue.
            let plan = plan_order_recovery(b.local_evidence(), V::Absent);
            assert!(!plan.reason.is_empty(), "{b:?}");
        }
        // The safety-critical ones:
        assert_eq!(
            plan_order_recovery(C::BeforeSubmission.local_evidence(), V::Absent).action,
            A::CloseUnsent,
            "a crash before the send never loses money"
        );
        assert_eq!(
            plan_order_recovery(C::AfterSubmission.local_evidence(), V::Absent).action,
            A::HoldAmbiguous,
            "a crash after the send is ambiguous, never a resubmit"
        );
        assert_eq!(
            plan_order_recovery(
                C::AfterVenueAck.local_evidence(),
                V::Filled { matched: 1.0 }
            )
            .action,
            A::FinalizeFilled
        );
        assert_eq!(
            plan_order_recovery(
                C::AfterFillAccounting.local_evidence(),
                V::Filled { matched: 1.0 }
            )
            .action,
            A::FinalizeFilled,
            "re-booking is safe: the ledger's event id makes it a duplicate"
        );
    }

    #[test]
    fn action_labels_round_trip_and_classify() {
        for a in A::ALL {
            assert_eq!(A::parse(a.as_str()), Some(a));
        }
        assert!(A::FinalizeFilled.books_money());
        assert!(A::ResumeTracking.books_money());
        assert!(!A::CloseUnsent.books_money());
        assert!(A::HoldAmbiguous.needs_reconcile());
        assert!(A::AdoptFromVenue.needs_reconcile());
        assert!(!A::NoAction.needs_reconcile());
    }
}
