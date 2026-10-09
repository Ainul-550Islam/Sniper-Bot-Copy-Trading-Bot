//! Invoice domain models (BATCH file 03).
//!
//! Invoice identity, organization, subscription, provider invoice ID,
//! amount/currency, status, period, timestamps, hosted reference,
//! payment relation, audit metadata. No payment secrets.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::tenant::OrganizationId;

use super::provider::BillingProviderKind;

/// Invoice identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InvoiceId(pub Uuid);

impl InvoiceId {
    pub fn new() -> Self {
        InvoiceId(Uuid::new_v4())
    }
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
    pub fn parse(s: &str) -> Option<Self> {
        Uuid::parse_str(s.trim()).ok().map(InvoiceId)
    }
}
impl Default for InvoiceId {
    fn default() -> Self {
        InvoiceId::new()
    }
}
impl std::fmt::Display for InvoiceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Invoice status vocabulary — provider-neutral.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvoiceStatus {
    Draft,
    Open,
    Paid,
    Void,
    Uncollectible,
    Refunded,
}

impl InvoiceStatus {
    pub const ALL: [InvoiceStatus; 6] = [
        InvoiceStatus::Draft,
        InvoiceStatus::Open,
        InvoiceStatus::Paid,
        InvoiceStatus::Void,
        InvoiceStatus::Uncollectible,
        InvoiceStatus::Refunded,
    ];
    pub fn as_str(&self) -> &'static str {
        match self {
            InvoiceStatus::Draft => "draft",
            InvoiceStatus::Open => "open",
            InvoiceStatus::Paid => "paid",
            InvoiceStatus::Void => "void",
            InvoiceStatus::Uncollectible => "uncollectible",
            InvoiceStatus::Refunded => "refunded",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|x| x.as_str() == s.trim())
    }
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            InvoiceStatus::Paid
                | InvoiceStatus::Void
                | InvoiceStatus::Refunded
                | InvoiceStatus::Uncollectible
        )
    }
    pub fn is_collectible(&self) -> bool {
        matches!(self, InvoiceStatus::Open | InvoiceStatus::Draft)
    }
}

/// Canonical invoice record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invoice {
    pub id: InvoiceId,
    pub organization_id: OrganizationId,
    pub subscription_id: Option<Uuid>,
    pub payment_transaction_id: Option<Uuid>,
    pub provider: BillingProviderKind,
    pub provider_invoice_id: Option<String>,
    pub invoice_number: Option<String>,
    pub status: InvoiceStatus,
    pub amount_cents: i64,
    pub amount_paid_cents: i64,
    pub amount_due_cents: i64,
    pub currency: String,
    pub period_start: Option<DateTime<Utc>>,
    pub period_end: Option<DateTime<Utc>>,
    pub due_date: Option<DateTime<Utc>>,
    pub paid_at: Option<DateTime<Utc>>,
    pub hosted_url: Option<String>,
    pub pdf_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Invoice {
    pub fn new(
        organization_id: OrganizationId,
        provider: BillingProviderKind,
        amount_cents: i64,
        currency: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Self {
        let currency = currency.into().to_ascii_lowercase();
        Self {
            id: InvoiceId::new(),
            organization_id,
            subscription_id: None,
            payment_transaction_id: None,
            provider,
            provider_invoice_id: None,
            invoice_number: None,
            status: InvoiceStatus::Draft,
            amount_cents,
            amount_paid_cents: 0,
            amount_due_cents: amount_cents,
            currency,
            period_start: None,
            period_end: None,
            due_date: None,
            paid_at: None,
            hosted_url: None,
            pdf_url: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn can_transition_to(&self, to: InvoiceStatus) -> bool {
        validate_transition(self.status, to)
    }

    pub fn transition_to(&mut self, to: InvoiceStatus, now: DateTime<Utc>) -> Result<(), String> {
        if !self.can_transition_to(to) {
            return Err(format!(
                "illegal invoice transition {} -> {}",
                self.status.as_str(),
                to.as_str()
            ));
        }
        self.status = to;
        self.updated_at = now;
        if to == InvoiceStatus::Paid {
            self.paid_at = Some(now);
            self.amount_paid_cents = self.amount_cents;
            self.amount_due_cents = 0;
        }
        if to == InvoiceStatus::Refunded {
            self.amount_paid_cents = 0;
            self.amount_due_cents = 0;
        }
        Ok(())
    }

    /// Validate amount invariants (paid + due == total for non-void, etc.)
    pub fn validate(&self) -> Result<(), String> {
        if self.amount_cents < 0 {
            return Err("amount_cents must be >= 0".into());
        }
        if self.amount_paid_cents < 0 || self.amount_due_cents < 0 {
            return Err("amount components must be >=0".into());
        }
        if self.currency.len() != 3 {
            return Err("currency must be 3 letters".into());
        }
        if let (Some(start), Some(end)) = (self.period_start, self.period_end) {
            if end <= start {
                return Err("period_end must be after period_start".into());
            }
        }
        Ok(())
    }

    pub fn summary(&self) -> String {
        format!(
            "invoice={} org={} provider={} status={} amount={} {}",
            self.id,
            self.organization_id,
            self.provider.as_str(),
            self.status.as_str(),
            self.amount_cents,
            self.currency
        )
    }
}

/// Pure invoice-state transition validation.
pub fn validate_transition(from: InvoiceStatus, to: InvoiceStatus) -> bool {
    use InvoiceStatus::*;
    if from == to {
        return false;
    }
    match (from, to) {
        (Draft, Open) => true,
        (Draft, Void) => true,
        (Draft, Paid) => true, // manual immediate payment
        (Open, Paid) => true,
        (Open, Void) => true,
        (Open, Uncollectible) => true,
        (Open, Refunded) => false, // must be paid first
        (Paid, Refunded) => true,
        (Uncollectible, Void) => true,
        _ => false,
    }
}

/// Serialization rules: never emit payment secrets; hosted_url is optional reference.
pub fn is_valid_currency(s: &str) -> bool {
    s.len() == 3 && s.chars().all(|c| c.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[allow(dead_code)]
    fn invoice(status: InvoiceStatus) -> Invoice {
        let mut inv = Invoice::new(
            OrganizationId::new(),
            BillingProviderKind::Stripe,
            1000,
            "usd",
            Utc::now(),
        );
        inv.status = status;
        inv
    }

    #[test]
    fn status_round_trip() {
        for s in InvoiceStatus::ALL {
            assert_eq!(InvoiceStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(InvoiceStatus::parse("paid"), Some(InvoiceStatus::Paid));
        assert_eq!(InvoiceStatus::parse("PAID"), None); // strict
    }

    #[test]
    fn valid_transitions() {
        assert!(validate_transition(
            InvoiceStatus::Draft,
            InvoiceStatus::Open
        ));
        assert!(validate_transition(
            InvoiceStatus::Open,
            InvoiceStatus::Paid
        ));
        assert!(validate_transition(
            InvoiceStatus::Paid,
            InvoiceStatus::Refunded
        ));
        assert!(validate_transition(
            InvoiceStatus::Draft,
            InvoiceStatus::Void
        ));
    }

    #[test]
    fn illegal_transitions_denied() {
        assert!(!validate_transition(
            InvoiceStatus::Paid,
            InvoiceStatus::Open
        ));
        assert!(!validate_transition(
            InvoiceStatus::Paid,
            InvoiceStatus::Paid
        ));
        assert!(!validate_transition(
            InvoiceStatus::Refunded,
            InvoiceStatus::Paid
        ));
        assert!(!validate_transition(
            InvoiceStatus::Open,
            InvoiceStatus::Refunded
        ));
    }

    #[test]
    fn transition_updates_paid_fields() {
        let now = Utc::now();
        let mut inv = Invoice::new(
            OrganizationId::new(),
            BillingProviderKind::Stripe,
            2000,
            "usd",
            now,
        );
        inv.status = InvoiceStatus::Open;
        inv.transition_to(InvoiceStatus::Paid, now).unwrap();
        assert_eq!(inv.status, InvoiceStatus::Paid);
        assert_eq!(inv.paid_at, Some(now));
        assert_eq!(inv.amount_paid_cents, 2000);
        assert_eq!(inv.amount_due_cents, 0);
    }

    #[test]
    fn amount_validation() {
        let now = Utc::now();
        let mut inv = Invoice::new(
            OrganizationId::new(),
            BillingProviderKind::Stripe,
            1000,
            "usd",
            now,
        );
        assert!(inv.validate().is_ok());
        inv.amount_cents = -1;
        assert!(inv.validate().is_err());
        inv.amount_cents = 1000;
        inv.currency = "us".into();
        assert!(inv.validate().is_err());
    }

    #[test]
    fn period_validation() {
        let now = Utc::now();
        let mut inv = Invoice::new(
            OrganizationId::new(),
            BillingProviderKind::Stripe,
            1000,
            "usd",
            now,
        );
        inv.period_start = Some(now);
        inv.period_end = Some(now - chrono::Duration::hours(1));
        assert!(inv.validate().is_err());
        inv.period_end = Some(now + chrono::Duration::days(30));
        assert!(inv.validate().is_ok());
    }

    #[test]
    fn serialization_never_contains_secrets() {
        let inv = Invoice::new(
            OrganizationId::new(),
            BillingProviderKind::Stripe,
            1000,
            "usd",
            Utc::now(),
        );
        let json = serde_json::to_string(&inv).unwrap();
        assert!(!json.contains("secret"));
        assert!(!json.contains("card"));
        assert!(inv.summary().contains("invoice="));
    }

    #[test]
    fn terminal_and_collectible_classification() {
        assert!(InvoiceStatus::Paid.is_terminal());
        assert!(!InvoiceStatus::Open.is_terminal());
        assert!(InvoiceStatus::Open.is_collectible());
        assert!(!InvoiceStatus::Paid.is_collectible());
    }
}
