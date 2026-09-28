//! Canonical provider-event normalization layer (BATCH 2 file 01).
//!
//! Converts provider-specific webhook payloads (Stripe, Paddle, Manual)
//! into internal `ProviderNormalizedEvent`s. The normalized form is
//! provider-agnostic and never retains secret/signature material.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::tenant::OrganizationId;

use super::provider::BillingProviderKind;

/// Canonical internal event kind — 9 vocabulary entries + unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderEventKind {
    PaymentSucceeded,
    PaymentFailed,
    SubscriptionCreated,
    SubscriptionUpdated,
    SubscriptionCanceled,
    InvoiceCreated,
    InvoicePaid,
    InvoiceFailed,
    RefundCreated,
    Unknown,
}

impl ProviderEventKind {
    pub const ALL: [ProviderEventKind; 9] = [
        ProviderEventKind::PaymentSucceeded,
        ProviderEventKind::PaymentFailed,
        ProviderEventKind::SubscriptionCreated,
        ProviderEventKind::SubscriptionUpdated,
        ProviderEventKind::SubscriptionCanceled,
        ProviderEventKind::InvoiceCreated,
        ProviderEventKind::InvoicePaid,
        ProviderEventKind::InvoiceFailed,
        ProviderEventKind::RefundCreated,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderEventKind::PaymentSucceeded => "payment_succeeded",
            ProviderEventKind::PaymentFailed => "payment_failed",
            ProviderEventKind::SubscriptionCreated => "subscription_created",
            ProviderEventKind::SubscriptionUpdated => "subscription_updated",
            ProviderEventKind::SubscriptionCanceled => "subscription_canceled",
            ProviderEventKind::InvoiceCreated => "invoice_created",
            ProviderEventKind::InvoicePaid => "invoice_paid",
            ProviderEventKind::InvoiceFailed => "invoice_failed",
            ProviderEventKind::RefundCreated => "refund_created",
            ProviderEventKind::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Self {
        let n = s.trim().to_ascii_lowercase();
        match n.as_str() {
            "payment_succeeded" | "payment.succeeded" | "charge.succeeded" => {
                ProviderEventKind::PaymentSucceeded
            }
            "payment_failed" | "payment.failed" | "charge.failed" => {
                ProviderEventKind::PaymentFailed
            }
            "subscription_created" | "subscription.created" => {
                ProviderEventKind::SubscriptionCreated
            }
            "subscription_updated" | "subscription.updated" | "subscription.renewed" => {
                ProviderEventKind::SubscriptionUpdated
            }
            "subscription_canceled"
            | "subscription.canceled"
            | "subscription.cancelled"
            | "customer.subscription.deleted" => ProviderEventKind::SubscriptionCanceled,
            "invoice_created" | "invoice.created" => ProviderEventKind::InvoiceCreated,
            "invoice_paid" | "invoice.paid" | "invoice.payment_succeeded" => {
                ProviderEventKind::InvoicePaid
            }
            "invoice_failed" | "invoice.payment_failed" => ProviderEventKind::InvoiceFailed,
            "refund_created" | "refund.created" | "charge.refunded" | "payment.refunded" => {
                ProviderEventKind::RefundCreated
            }
            _ => ProviderEventKind::Unknown,
        }
    }

    pub fn is_known(&self) -> bool {
        !matches!(self, ProviderEventKind::Unknown)
    }
}

/// Idempotency identity for deduplication: (provider, provider_event_id).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventIdempotencyKey {
    pub provider: BillingProviderKind,
    pub provider_event_id: String,
}

impl EventIdempotencyKey {
    pub fn new(provider: BillingProviderKind, provider_event_id: impl Into<String>) -> Self {
        Self {
            provider,
            provider_event_id: provider_event_id.into(),
        }
    }
    pub fn as_key(&self) -> String {
        format!("{}:{}", self.provider.as_str(), self.provider_event_id)
    }
}

/// Normalized payload metadata — only safe fields, no secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedPayloadMeta {
    /// Original provider event type string (preserved for audit).
    pub raw_type: String,
    /// Optional provider object id (payment_intent, subscription, invoice).
    pub provider_object_id: Option<String>,
    /// Optional amount in cents (if applicable).
    pub amount_cents: Option<i64>,
    /// Optional currency (lowercased).
    pub currency: Option<String>,
    /// Extra safe key-values (no secret keys).
    pub extra: std::collections::BTreeMap<String, String>,
}

impl NormalizedPayloadMeta {
    pub fn empty(raw_type: impl Into<String>) -> Self {
        Self {
            raw_type: raw_type.into(),
            provider_object_id: None,
            amount_cents: None,
            currency: None,
            extra: Default::default(),
        }
    }
}

/// Canonical normalized provider event — never retains secret/signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderNormalizedEvent {
    pub id: Uuid,
    pub provider: BillingProviderKind,
    pub provider_event_id: String,
    pub kind: ProviderEventKind,
    /// Tenant mapping — may be None if provider payload lacks organization link; caller must resolve.
    pub organization_id: Option<OrganizationId>,
    /// Customer mapping — provider customer id if present.
    pub provider_customer_id: Option<String>,
    pub event_timestamp: DateTime<Utc>,
    pub received_at: DateTime<Utc>,
    pub idempotency: EventIdempotencyKey,
    pub meta: NormalizedPayloadMeta,
    /// Minimal safe payload (secrets stripped).
    pub safe_payload: serde_json::Value,
}

impl ProviderNormalizedEvent {
    /// Create a normalized event. `provider_event_id` must be non-empty.
    pub fn new(
        provider: BillingProviderKind,
        provider_event_id: impl Into<String>,
        kind: ProviderEventKind,
        event_timestamp: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<Self, String> {
        let pid = provider_event_id.into();
        if pid.trim().is_empty() {
            return Err("provider_event_id must not be empty".into());
        }
        let idem = EventIdempotencyKey::new(provider, pid.clone());
        Ok(Self {
            id: Uuid::new_v4(),
            provider,
            provider_event_id: pid,
            kind,
            organization_id: None,
            provider_customer_id: None,
            event_timestamp,
            received_at: now,
            idempotency: idem,
            meta: NormalizedPayloadMeta::empty(kind.as_str()),
            safe_payload: serde_json::Value::Object(Default::default()),
        })
    }

    pub fn with_organization(mut self, org: OrganizationId) -> Self {
        self.organization_id = Some(org);
        self
    }

    pub fn with_customer(mut self, customer_id: impl Into<String>) -> Self {
        self.provider_customer_id = Some(customer_id.into());
        self
    }

    pub fn with_meta(mut self, meta: NormalizedPayloadMeta) -> Self {
        self.meta = meta;
        self
    }

    pub fn with_safe_payload(mut self, payload: serde_json::Value) -> Self {
        // Strip any secret-looking keys before storing
        let stripped = strip_secrets(payload);
        self.safe_payload = stripped;
        self
    }
}

/// Strip secret-bearing keys from a JSON value (recursive).
pub fn strip_secrets(value: serde_json::Value) -> serde_json::Value {
    const BANNED_SUBSTRINGS: &[&str] = &[
        "secret",
        "signature",
        "token",
        "password",
        "api_key",
        "apikey",
        "private_key",
        "webhook_secret",
    ];
    match value {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                let lower = k.to_ascii_lowercase();
                if BANNED_SUBSTRINGS.iter().any(|b| lower.contains(b)) {
                    continue;
                }
                out.insert(k, strip_secrets(v));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(strip_secrets).collect())
        }
        other => other,
    }
}

/// Normalize a raw provider payload into a `ProviderNormalizedEvent`.
///
/// `raw_type` is the provider's event type string, `raw_payload` is the
/// JSON body (already signature-verified). This function never retains
/// `signature`, `secret`, or any header material — only safe fields.
pub fn normalize(
    provider: BillingProviderKind,
    provider_event_id: impl Into<String>,
    raw_type: impl Into<String>,
    raw_payload: &serde_json::Value,
    event_timestamp: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<ProviderNormalizedEvent, String> {
    let raw_type_s = raw_type.into();
    let kind = ProviderEventKind::parse(&raw_type_s);
    let pid = provider_event_id.into();
    if pid.trim().is_empty() {
        return Err("provider_event_id empty".into());
    }
    let org = raw_payload
        .get("organization_id")
        .or_else(|| {
            raw_payload
                .get("data")
                .and_then(|d| d.get("organization_id"))
        })
        .and_then(|v| v.as_str())
        .and_then(OrganizationId::parse);

    let customer = raw_payload
        .get("customer")
        .or_else(|| raw_payload.get("data").and_then(|d| d.get("customer")))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let provider_object_id = raw_payload
        .get("id")
        .or_else(|| raw_payload.get("data").and_then(|d| d.get("id")))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let amount_cents = raw_payload
        .get("amount_cents")
        .or_else(|| raw_payload.get("data").and_then(|d| d.get("amount_cents")))
        .and_then(|v| v.as_i64());

    let currency = raw_payload
        .get("currency")
        .or_else(|| raw_payload.get("data").and_then(|d| d.get("currency")))
        .and_then(|v| v.as_str())
        .map(|s| s.to_ascii_lowercase());

    let mut extra = std::collections::BTreeMap::new();
    if let Some(obj) = raw_payload.get("data").and_then(|d| d.as_object()) {
        for (k, v) in obj.iter().take(10) {
            if let Some(s) = v.as_str() {
                let lower = k.to_ascii_lowercase();
                if lower.contains("secret") || lower.contains("signature") {
                    continue;
                }
                extra.insert(k.clone(), s.chars().take(256).collect());
            }
        }
    }

    let meta = NormalizedPayloadMeta {
        raw_type: raw_type_s.clone(),
        provider_object_id,
        amount_cents,
        currency,
        extra,
    };

    let mut ev = ProviderNormalizedEvent::new(provider, pid, kind, event_timestamp, now)?;
    if let Some(o) = org {
        ev = ev.with_organization(o);
    }
    if let Some(c) = customer {
        ev = ev.with_customer(c);
    }
    ev = ev.with_meta(meta);
    ev = ev.with_safe_payload(raw_payload.clone());
    Ok(ev)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn kind_parse_vocabulary() {
        assert_eq!(
            ProviderEventKind::parse("payment_succeeded"),
            ProviderEventKind::PaymentSucceeded
        );
        assert_eq!(
            ProviderEventKind::parse("payment.succeeded"),
            ProviderEventKind::PaymentSucceeded
        );
        assert_eq!(
            ProviderEventKind::parse("invoice.paid"),
            ProviderEventKind::InvoicePaid
        );
        assert_eq!(
            ProviderEventKind::parse("subscription.canceled"),
            ProviderEventKind::SubscriptionCanceled
        );
        assert_eq!(
            ProviderEventKind::parse("unknown_xyz"),
            ProviderEventKind::Unknown
        );
        for k in ProviderEventKind::ALL {
            assert!(k.is_known());
            assert_eq!(ProviderEventKind::parse(k.as_str()), k);
        }
    }

    #[test]
    fn idempotency_key_is_deterministic() {
        let k1 = EventIdempotencyKey::new(BillingProviderKind::Stripe, "evt_123");
        let k2 = EventIdempotencyKey::new(BillingProviderKind::Stripe, "evt_123");
        assert_eq!(k1.as_key(), k2.as_key());
        assert_eq!(k1.as_key(), "stripe:evt_123");
        let k3 = EventIdempotencyKey::new(BillingProviderKind::Paddle, "evt_123");
        assert_ne!(k1.as_key(), k3.as_key());
    }

    #[test]
    fn normalized_event_rejects_empty_provider_id() {
        let now = Utc::now();
        let r = ProviderNormalizedEvent::new(
            BillingProviderKind::Stripe,
            "  ",
            ProviderEventKind::PaymentSucceeded,
            now,
            now,
        );
        assert!(r.is_err());
    }

    #[test]
    fn strip_secrets_removes_sensitive_keys() {
        let payload = serde_json::json!({
            "id": "evt_123",
            "secret": "should_be_removed",
            "nested": {
                "api_key": "remove",
                "public": "keep"
            },
            "amount_cents": 1000
        });
        let stripped = strip_secrets(payload);
        assert!(stripped.get("secret").is_none());
        assert!(stripped["nested"].get("api_key").is_none());
        assert_eq!(stripped["nested"]["public"], "keep");
        assert_eq!(stripped["id"], "evt_123");
    }

    #[test]
    fn normalize_strips_secrets_and_preserves_org() {
        let now = Utc::now();
        let org = OrganizationId::new();
        let payload = serde_json::json!({
            "organization_id": org.to_string(),
            "customer": "cus_123",
            "amount_cents": 5000,
            "currency": "USD",
            "data": {
                "id": "in_123"
            },
            "webhook_secret": "should_not_persist"
        });
        let ev = normalize(
            BillingProviderKind::Stripe,
            "evt_999",
            "invoice.paid",
            &payload,
            now,
            now,
        )
        .unwrap();
        assert_eq!(ev.kind, ProviderEventKind::InvoicePaid);
        assert_eq!(ev.organization_id, Some(org));
        assert_eq!(ev.meta.currency, Some("usd".into()));
        assert_eq!(ev.meta.amount_cents, Some(5000));
        assert!(ev.safe_payload.get("webhook_secret").is_none());
        assert!(ev.safe_payload.get("secret").is_none());
    }

    #[test]
    fn duplicate_provider_event_same_idempotency() {
        let now = Utc::now();
        let payload = serde_json::json!({"organization_id": OrganizationId::new().to_string()});
        let e1 = normalize(
            BillingProviderKind::Stripe,
            "evt_dup",
            "payment_succeeded",
            &payload,
            now,
            now,
        )
        .unwrap();
        let e2 = normalize(
            BillingProviderKind::Stripe,
            "evt_dup",
            "payment_succeeded",
            &payload,
            now,
            now,
        )
        .unwrap();
        assert_eq!(e1.idempotency.as_key(), e2.idempotency.as_key());
    }

    #[test]
    fn invalid_signature_not_in_normalized_event() {
        let payload = serde_json::json!({
            "id": "evt_123",
            "signature": "sig_abc",
            "type": "payment_succeeded"
        });
        let stripped = strip_secrets(payload);
        assert!(stripped.get("signature").is_none());
    }
}
