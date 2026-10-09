//! Canonical payment domain models and state machine (BATCH file 02).
//!
//! Financial truth separate from usage metering. Duplicate provider events
//! cannot create duplicate money states — idempotency is enforced via
//! `provider_event_id` and `idempotency_key`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::tenant::OrganizationId;

use super::provider::{BillingProviderKind, PaymentIntentStatus};

/// Unique payment transaction identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PaymentId(pub Uuid);

impl PaymentId {
    pub fn new() -> Self {
        PaymentId(Uuid::new_v4())
    }
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
    pub fn parse(s: &str) -> Option<Self> {
        Uuid::parse_str(s.trim()).ok().map(PaymentId)
    }
}
impl Default for PaymentId {
    fn default() -> Self {
        PaymentId::new()
    }
}
impl std::fmt::Display for PaymentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Provider event identity — the durable deduplication key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderEventIdentity {
    pub provider: BillingProviderKind,
    pub provider_event_id: String,
}

impl ProviderEventIdentity {
    pub fn new(provider: BillingProviderKind, provider_event_id: impl Into<String>) -> Self {
        Self {
            provider,
            provider_event_id: provider_event_id.into(),
        }
    }
    pub fn idempotency_key(&self) -> String {
        format!("{}:{}", self.provider.as_str(), self.provider_event_id)
    }
}

/// Payment transaction status — mirrors PaymentIntentStatus but is the canonical stored state.
pub type TransactionStatus = PaymentIntentStatus;

/// Failure classification for retry decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    /// Transient: network, timeout, 5xx — retry with same idempotency key.
    Retryable,
    /// Permanent: card declined, invalid request — do not retry same payload.
    Permanent,
    /// Requires user action (3DS, etc.)
    RequiresAction,
}

impl FailureClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            FailureClass::Retryable => "retryable",
            FailureClass::Permanent => "permanent",
            FailureClass::RequiresAction => "requires_action",
        }
    }
}

/// Classify a failure code string into retryability.
pub fn classify_failure(code: &str) -> FailureClass {
    let lower = code.trim().to_ascii_lowercase();
    match lower.as_str() {
        "card_declined" | "insufficient_funds" | "incorrect_cvc" | "expired_card"
        | "processing_error" => FailureClass::Permanent,
        "requires_action" | "requires_source_action" | "authentication_required" => {
            FailureClass::RequiresAction
        }
        "rate_limit"
        | "timeout"
        | "api_connection_error"
        | "api_error"
        | "idempotency_key_in_use" => FailureClass::Retryable,
        _ => {
            if lower.contains("timeout") || lower.contains("rate") || lower.contains("network") {
                FailureClass::Retryable
            } else {
                FailureClass::Permanent
            }
        }
    }
}

/// Canonical payment transaction — organization-owned money state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentTransaction {
    pub id: PaymentId,
    pub organization_id: OrganizationId,
    pub subscription_id: Option<Uuid>,
    pub checkout_session_id: Option<Uuid>,
    pub invoice_id: Option<Uuid>,
    pub provider: BillingProviderKind,
    pub provider_payment_id: Option<String>,
    pub idempotency_key: String,
    pub amount_cents: i64,
    pub currency: String,
    pub status: TransactionStatus,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub retryable: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub succeeded_at: Option<DateTime<Utc>>,
    pub failed_at: Option<DateTime<Utc>>,
}

impl PaymentTransaction {
    pub fn new(
        organization_id: OrganizationId,
        provider: BillingProviderKind,
        idempotency_key: impl Into<String>,
        amount_cents: i64,
        currency: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            id: PaymentId::new(),
            organization_id,
            subscription_id: None,
            checkout_session_id: None,
            invoice_id: None,
            provider,
            provider_payment_id: None,
            idempotency_key: idempotency_key.into(),
            amount_cents,
            currency: currency.into().to_ascii_lowercase(),
            status: TransactionStatus::Pending,
            failure_code: None,
            failure_message: None,
            retryable: false,
            created_at: now,
            updated_at: now,
            succeeded_at: None,
            failed_at: None,
        }
    }

    /// Validate state transition. Pure function — no I/O, exhaustive.
    pub fn can_transition_to(&self, to: TransactionStatus) -> bool {
        can_transition(self.status, to)
    }

    /// Apply a transition, returning descriptive error on illegal move. Updates timestamps.
    pub fn transition_to(
        &mut self,
        to: TransactionStatus,
        reason: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        if !self.can_transition_to(to) {
            return Err(format!(
                "illegal payment transition {} -> {}: {}",
                self.status.as_str(),
                to.as_str(),
                reason.unwrap_or("no reason")
            ));
        }
        self.status = to;
        self.updated_at = now;
        match to {
            TransactionStatus::Succeeded | TransactionStatus::Authorized => {
                self.succeeded_at = Some(now);
                self.failed_at = None;
                self.retryable = false;
            }
            TransactionStatus::Failed | TransactionStatus::Canceled => {
                self.failed_at = Some(now);
                // retryable derived from failure_code classification
                if let Some(code) = &self.failure_code {
                    self.retryable = matches!(classify_failure(code), FailureClass::Retryable);
                }
            }
            TransactionStatus::Refunded | TransactionStatus::PartiallyRefunded => {
                self.retryable = false;
            }
            _ => {}
        }
        Ok(())
    }

    /// Summary for audit (secret-free).
    pub fn summary(&self) -> String {
        format!(
            "payment={} org={} provider={} status={} amount={} {}",
            self.id,
            self.organization_id,
            self.provider.as_str(),
            self.status.as_str(),
            self.amount_cents,
            self.currency
        )
    }
}

/// Pure transition validation. Exhaustive — every legal edge is listed.
pub fn can_transition(from: TransactionStatus, to: TransactionStatus) -> bool {
    use crate::billing::provider::PaymentIntentStatus::*;
    if from == to {
        return false;
    } // idempotent replays handled at event layer, not as transition
    match (from, to) {
        // Pending can go anywhere except staying pending
        (Pending, RequiresAction) => true,
        (Pending, Authorized) => true,
        (Pending, Succeeded) => true,
        (Pending, Failed) => true,
        (Pending, Canceled) => true,
        // RequiresAction can succeed, fail, cancel
        (RequiresAction, Succeeded) => true,
        (RequiresAction, Failed) => true,
        (RequiresAction, Canceled) => true,
        (RequiresAction, Authorized) => true,
        // Authorized can be captured (succeeded) or voided
        (Authorized, Succeeded) => true,
        (Authorized, Failed) => true,
        (Authorized, Canceled) => true,
        // Succeeded can be refunded
        (Succeeded, Refunded) => true,
        (Succeeded, PartiallyRefunded) => true,
        // PartiallyRefunded can be fully refunded
        (PartiallyRefunded, Refunded) => true,
        // Failed / Canceled / Refunded are terminal (no outgoing except idempotent handled outside)
        _ => false,
    }
}

/// Organization ownership check: does this payment belong to org?
pub fn owns(payment: &PaymentTransaction, org: OrganizationId) -> bool {
    payment.organization_id == org
}

/// Duplicate event check: has this provider event already been recorded?
/// This is the pure helper; durable check is via provider_events table.
pub fn is_duplicate_event(
    seen: &std::collections::HashSet<String>,
    identity: &ProviderEventIdentity,
) -> bool {
    seen.contains(&identity.idempotency_key())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[allow(dead_code)]
    fn tx(status: TransactionStatus) -> PaymentTransaction {
        let mut t = PaymentTransaction::new(
            OrganizationId::new(),
            BillingProviderKind::Stripe,
            "idem-1",
            1000,
            "usd",
            Utc::now(),
        );
        t.status = status;
        t
    }

    #[test]
    fn valid_transitions() {
        assert!(can_transition(
            TransactionStatus::Pending,
            TransactionStatus::Succeeded
        ));
        assert!(can_transition(
            TransactionStatus::Pending,
            TransactionStatus::RequiresAction
        ));
        assert!(can_transition(
            TransactionStatus::RequiresAction,
            TransactionStatus::Succeeded
        ));
        assert!(can_transition(
            TransactionStatus::Authorized,
            TransactionStatus::Succeeded
        ));
        assert!(can_transition(
            TransactionStatus::Succeeded,
            TransactionStatus::Refunded
        ));
        assert!(can_transition(
            TransactionStatus::Succeeded,
            TransactionStatus::PartiallyRefunded
        ));
        assert!(can_transition(
            TransactionStatus::PartiallyRefunded,
            TransactionStatus::Refunded
        ));
    }

    #[test]
    fn illegal_transitions_are_denied() {
        assert!(!can_transition(
            TransactionStatus::Succeeded,
            TransactionStatus::Pending
        ));
        assert!(!can_transition(
            TransactionStatus::Failed,
            TransactionStatus::Succeeded
        ));
        assert!(!can_transition(
            TransactionStatus::Canceled,
            TransactionStatus::Succeeded
        ));
        assert!(!can_transition(
            TransactionStatus::Refunded,
            TransactionStatus::Succeeded
        ));
        assert!(!can_transition(
            TransactionStatus::Pending,
            TransactionStatus::Pending
        ));
        assert!(!can_transition(
            TransactionStatus::Succeeded,
            TransactionStatus::Failed
        ));
    }

    #[allow(unused_imports)]
    fn _use_status() {
        let _ = TransactionStatus::Pending;
    }

    #[test]
    fn transition_updates_timestamps() {
        let now = Utc::now();
        let mut t = PaymentTransaction::new(
            OrganizationId::new(),
            BillingProviderKind::Stripe,
            "idem-2",
            5000,
            "usd",
            now,
        );
        assert_eq!(t.status, TransactionStatus::Pending);
        t.transition_to(TransactionStatus::Succeeded, None, now)
            .unwrap();
        assert_eq!(t.status, TransactionStatus::Succeeded);
        assert_eq!(t.succeeded_at, Some(now));
        assert!(t
            .transition_to(TransactionStatus::Pending, None, now)
            .is_err());
    }

    #[test]
    fn failure_classification() {
        assert_eq!(classify_failure("card_declined"), FailureClass::Permanent);
        assert_eq!(classify_failure("timeout"), FailureClass::Retryable);
        assert_eq!(
            classify_failure("requires_action"),
            FailureClass::RequiresAction
        );
        assert_eq!(classify_failure("rate_limit"), FailureClass::Retryable);
    }

    #[test]
    fn duplicate_detection_is_pure() {
        let mut seen = std::collections::HashSet::new();
        let id = ProviderEventIdentity::new(BillingProviderKind::Stripe, "evt_123");
        assert!(!is_duplicate_event(&seen, &id));
        seen.insert(id.idempotency_key());
        assert!(is_duplicate_event(&seen, &id));
    }

    #[test]
    fn organization_ownership() {
        let org = OrganizationId::new();
        let mut t = PaymentTransaction::new(
            org,
            BillingProviderKind::Stripe,
            "k",
            100,
            "usd",
            Utc::now(),
        );
        assert!(owns(&t, org));
        assert!(!owns(&t, OrganizationId::new()));
        t.organization_id = org;
        assert!(t.summary().contains(&org.to_string()));
        assert!(
            !t.summary().contains("sk_"),
            "summary must not contain secrets"
        );
    }

    #[test]
    fn idempotency_key_is_stable() {
        let id = ProviderEventIdentity::new(BillingProviderKind::Paddle, "evt_999");
        assert_eq!(id.idempotency_key(), "paddle:evt_999");
    }

    #[test]
    fn payment_summary_is_secret_free() {
        let t = PaymentTransaction::new(
            OrganizationId::new(),
            BillingProviderKind::Stripe,
            "idem",
            1000,
            "usd",
            Utc::now(),
        );
        let s = t.summary();
        assert!(!s.contains("secret"));
        assert!(!s.contains("token"));
    }
}
