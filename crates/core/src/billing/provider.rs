//! Provider-neutral payment gateway abstraction (BATCH file 01).
//!
//! Defines the trait and types a concrete processor (Stripe/Paddle/Manual)
//! implements behind. The core billing domain never imports a provider SDK;
//! adapters live outside and plug in through this boundary.
//!
//! Secrets are never exposed: the trait takes configuration by value that
//! the caller has already resolved, and errors are typed without leaking
//! credential material.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::tenant::OrganizationId;

use super::plan::PlanCode;

/// Provider-neutral customer creation request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateCustomerRequest {
    pub organization_id: OrganizationId,
    pub email: String,
    pub display_name: String,
    /// Caller-supplied idempotency key (per organization). Must be stable for retries.
    pub idempotency_key: String,
}

/// Provider-neutral customer response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Customer {
    pub organization_id: OrganizationId,
    pub provider: BillingProviderKind,
    pub provider_customer_id: String,
    pub email: String,
    pub created_at: DateTime<Utc>,
}

/// Checkout/session creation request. Price authority is server-side plan definitions;
/// callers supply only plan_code, not amounts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateCheckoutRequest {
    pub organization_id: OrganizationId,
    pub plan_code: PlanCode,
    pub success_url: Option<String>,
    pub cancel_url: Option<String>,
    pub idempotency_key: String,
}

/// Checkout/session response. Never contains secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckoutSession {
    pub id: Uuid,
    pub organization_id: OrganizationId,
    pub provider: BillingProviderKind,
    pub provider_session_id: Option<String>,
    /// For providers with hosted checkout: the URL to redirect to.
    pub checkout_url: Option<String>,
    /// Human instruction when no hosted flow exists (manual).
    pub instructions: Option<String>,
    pub status: CheckoutStatus,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckoutStatus {
    Pending,
    Open,
    Completed,
    Expired,
    Canceled,
}

impl CheckoutStatus {
    pub const ALL: [CheckoutStatus; 5] = [
        CheckoutStatus::Pending,
        CheckoutStatus::Open,
        CheckoutStatus::Completed,
        CheckoutStatus::Expired,
        CheckoutStatus::Canceled,
    ];
    pub fn as_str(&self) -> &'static str {
        match self {
            CheckoutStatus::Pending => "pending",
            CheckoutStatus::Open => "open",
            CheckoutStatus::Completed => "completed",
            CheckoutStatus::Expired => "expired",
            CheckoutStatus::Canceled => "canceled",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|x| x.as_str() == s.trim())
    }
}

/// Provider-neutral invoice view (read-only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceView {
    pub id: Uuid,
    pub organization_id: OrganizationId,
    pub provider: BillingProviderKind,
    pub provider_invoice_id: Option<String>,
    pub status: InvoiceStatus,
    pub amount_cents: i64,
    pub amount_paid_cents: i64,
    pub currency: String,
    pub period_start: Option<DateTime<Utc>>,
    pub period_end: Option<DateTime<Utc>>,
    pub hosted_url: Option<String>,
    pub created_at: DateTime<Utc>,
}

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

/// Provider-neutral payment status view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentStatus {
    pub provider_payment_id: Option<String>,
    pub status: PaymentIntentStatus,
    pub amount_cents: i64,
    pub currency: String,
    pub failure_code: Option<String>,
}

/// The vocabulary of provider-neutral billing backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingProviderKind {
    Manual,
    Stripe,
    Paddle,
}

impl BillingProviderKind {
    pub const ALL: [BillingProviderKind; 3] = [
        BillingProviderKind::Manual,
        BillingProviderKind::Stripe,
        BillingProviderKind::Paddle,
    ];
    pub fn as_str(&self) -> &'static str {
        match self {
            BillingProviderKind::Manual => "manual",
            BillingProviderKind::Stripe => "stripe",
            BillingProviderKind::Paddle => "paddle",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|x| x.as_str() == s.trim().to_ascii_lowercase())
    }
}

/// Normalized provider event after signature verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedEvent {
    pub provider: BillingProviderKind,
    pub provider_event_id: String,
    pub event_type: String,
    pub organization_id: Option<OrganizationId>,
    pub payload: serde_json::Value,
    pub received_at: DateTime<Utc>,
}

/// Why a provider operation failed. Never carries secret material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// Provider not configured or no adapter installed.
    UnsupportedProvider(String),
    /// Configuration missing/invalid (e.g., webhook secret absent).
    Configuration(String),
    /// Signature verification failed.
    VerificationFailed(String),
    /// Transport / processor unreachable.
    Transport(String),
    /// Provider returned an error classified as not-retryable or retryable but with detail.
    Provider(String),
    /// Idempotency or validation error before calling provider.
    InvalidRequest(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::UnsupportedProvider(p) => write!(f, "unsupported provider: {}", p),
            ProviderError::Configuration(m) => write!(f, "provider configuration error: {}", m),
            ProviderError::VerificationFailed(m) => write!(f, "webhook verification failed: {}", m),
            ProviderError::Transport(m) => write!(f, "provider transport error: {}", m),
            ProviderError::Provider(m) => write!(f, "provider error: {}", m),
            ProviderError::InvalidRequest(m) => write!(f, "invalid request: {}", m),
        }
    }
}

impl std::error::Error for ProviderError {}

impl ProviderError {
    /// Machine code for API mapping.
    pub fn code(&self) -> &'static str {
        match self {
            ProviderError::UnsupportedProvider(_) => "unsupported_provider",
            ProviderError::Configuration(_) => "provider_configuration",
            ProviderError::VerificationFailed(_) => "verification_failed",
            ProviderError::Transport(_) => "provider_transport",
            ProviderError::Provider(_) => "provider_error",
            ProviderError::InvalidRequest(_) => "invalid_request",
        }
    }
    /// Whether the caller may retry with same idempotency key after fixing external condition.
    pub fn is_retryable(&self) -> bool {
        matches!(self, ProviderError::Transport(_))
    }
}

/// The provider-neutral gateway trait. Async and testable; no secret material in errors/logs.
#[async_trait]
pub trait PaymentGateway: Send + Sync {
    fn provider(&self) -> BillingProviderKind;

    async fn create_customer(&self, req: CreateCustomerRequest) -> Result<Customer, ProviderError>;

    async fn create_checkout(
        &self,
        req: CreateCheckoutRequest,
    ) -> Result<CheckoutSession, ProviderError>;

    /// Synchronize subscription state from provider (polling fallback / reconciliation).
    async fn sync_subscription(
        &self,
        organization_id: OrganizationId,
    ) -> Result<Option<SubscriptionSync>, ProviderError>;

    async fn get_invoice(
        &self,
        provider_invoice_id: &str,
    ) -> Result<Option<InvoiceView>, ProviderError>;

    async fn get_payment_status(
        &self,
        provider_payment_id: &str,
    ) -> Result<Option<PaymentStatus>, ProviderError>;

    async fn cancel_subscription(
        &self,
        organization_id: OrganizationId,
        at_period_end: bool,
    ) -> Result<(), ProviderError>;

    async fn refund_payment(
        &self,
        provider_payment_id: &str,
        amount_cents: Option<i64>,
    ) -> Result<(), ProviderError>;

    /// Verify webhook signature and normalize event. Must not trust unverified payload.
    fn verify_and_normalize(
        &self,
        headers: &http::HeaderMap,
        body: &[u8],
        now: DateTime<Utc>,
    ) -> Result<NormalizedEvent, ProviderError>;
}

/// Result of a subscription synchronization poll.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionSync {
    pub organization_id: OrganizationId,
    pub provider: BillingProviderKind,
    pub provider_subscription_id: Option<String>,
    pub status: super::subscription::SubscriptionStatus,
    pub current_period_end: Option<DateTime<Utc>>,
    pub cancel_at_period_end: bool,
    pub synced_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentIntentStatus {
    Pending,
    RequiresAction,
    Authorized,
    Succeeded,
    Failed,
    Canceled,
    Refunded,
    PartiallyRefunded,
}

impl PaymentIntentStatus {
    pub const ALL: [PaymentIntentStatus; 8] = [
        PaymentIntentStatus::Pending,
        PaymentIntentStatus::RequiresAction,
        PaymentIntentStatus::Authorized,
        PaymentIntentStatus::Succeeded,
        PaymentIntentStatus::Failed,
        PaymentIntentStatus::Canceled,
        PaymentIntentStatus::Refunded,
        PaymentIntentStatus::PartiallyRefunded,
    ];
    pub fn as_str(&self) -> &'static str {
        match self {
            PaymentIntentStatus::Pending => "pending",
            PaymentIntentStatus::RequiresAction => "requires_action",
            PaymentIntentStatus::Authorized => "authorized",
            PaymentIntentStatus::Succeeded => "succeeded",
            PaymentIntentStatus::Failed => "failed",
            PaymentIntentStatus::Canceled => "canceled",
            PaymentIntentStatus::Refunded => "refunded",
            PaymentIntentStatus::PartiallyRefunded => "partially_refunded",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|x| x.as_str() == s.trim())
    }
    pub fn is_terminal_success(&self) -> bool {
        matches!(self, PaymentIntentStatus::Succeeded)
    }
    pub fn is_terminal_failure(&self) -> bool {
        matches!(
            self,
            PaymentIntentStatus::Failed | PaymentIntentStatus::Canceled
        )
    }
    pub fn is_retryable_failure(&self) -> bool {
        matches!(self, PaymentIntentStatus::Failed) // caller decides via FailureClass
    }
}

/// Minimal http HeaderMap re-export so trait is not tied to axum.
pub use http;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kinds_round_trip() {
        for p in BillingProviderKind::ALL {
            assert_eq!(BillingProviderKind::parse(p.as_str()), Some(p));
        }
        assert_eq!(
            BillingProviderKind::parse("STRIPE"),
            Some(BillingProviderKind::Stripe)
        );
        assert_eq!(BillingProviderKind::parse("unknown"), None);
    }

    #[test]
    fn checkout_status_vocabulary() {
        for s in CheckoutStatus::ALL {
            assert_eq!(CheckoutStatus::parse(s.as_str()), Some(s));
        }
    }

    #[test]
    fn provider_error_is_secret_free() {
        let e = ProviderError::Configuration("missing webhook secret for stripe".into());
        let text = e.to_string();
        assert!(text.contains("stripe"));
        assert!(
            !text.contains("sk_"),
            "error must not carry secret material"
        );
        assert_eq!(e.code(), "provider_configuration");
        assert!(!ProviderError::InvalidRequest("bad".into()).is_retryable());
        assert!(ProviderError::Transport("timeout".into()).is_retryable());
    }

    #[test]
    fn payment_intent_status_classification() {
        assert!(PaymentIntentStatus::Succeeded.is_terminal_success());
        assert!(PaymentIntentStatus::Failed.is_terminal_failure());
        assert!(!PaymentIntentStatus::Pending.is_terminal_success());
    }
}
