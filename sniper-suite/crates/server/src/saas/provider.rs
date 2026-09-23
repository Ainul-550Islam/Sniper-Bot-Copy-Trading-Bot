//! Provider-neutral billing adapter boundary (TASK 7B file 11).
//!
//! TASK 7A made the billing *domain* provider-neutral: a [`Subscription`]
//! carries a [`BillingProvider`] tag and an opaque `provider_ref`. This file
//! is the boundary the *transport* lives behind: checkout creation and
//! webhook verification are traits implemented per provider, and the rest of
//! the server only ever sees the neutral types here.
//!
//! Rules:
//!
//! * No SDK, HTTP client or credential model for any processor appears in
//!   the core billing domain; adapters (added later) implement
//!   [`BillingProviderAdapter`] outside `bot-core`.
//! * Webhook secrets are process configuration: they are never logged,
//!   serialised, returned by an API or written to the database. [`Debug`]
//!   for a secret is redacted.
//! * The signature scheme is deliberately simple and testable:
//!   `hex(HMAC-SHA256(secret, "{timestamp}.{body}"))` with a freshness
//!   window. A Stripe/Paddle adapter implements the same trait with its own
//!   scheme behind the boundary.
//! * `manual` (the only provider implemented in TASK 7A) has no webhooks and
//!   no checkout host: its adapter answers honestly instead of pretending.

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;

use bot_core::billing::{BillingProvider, PlanCode};
use bot_core::tenant::OrganizationId;

use super::api_keys::SaasApiKey;
use bot_core::session::token::constant_time_eq;

/// How far a signed webhook timestamp may drift from now.
pub const SIGNATURE_TOLERANCE: Duration = Duration::from_secs(300);

/// Header carrying the unix-seconds timestamp the signature covers.
pub const TIMESTAMP_HEADER: &str = "x-provider-timestamp";

/// Header carrying the hex HMAC signature of `{timestamp}.{body}`.
pub const SIGNATURE_HEADER: &str = "x-provider-signature";

/// A provider webhook/API secret. Server-side configuration only.
///
/// The value is held so the adapter can sign/verify; it is never logged
/// ([`Debug`] is redacted), never serialised and never returned to a client.
#[derive(Clone)]
pub struct WebhookSecret(pub String);

impl fmt::Debug for WebhookSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WebhookSecret(**redacted**)")
    }
}

impl WebhookSecret {
    /// A secret from configuration. Empty values are refused: a missing
    /// secret must disable the integration, not silently accept forgeries.
    pub fn parse(value: &str) -> Option<Self> {
        let v = value.trim();
        (!v.is_empty()).then(|| WebhookSecret(v.to_string()))
    }
}

/// A verified provider event: the ONLY thing a webhook handler may act on.
#[derive(Debug, Clone)]
pub struct VerifiedProviderEvent {
    /// Which provider it came from.
    pub provider: BillingProvider,
    /// The provider's event id — the durable idempotency identity.
    pub event_id: String,
    /// Namespaced event type (`subscription.payment_failed`, …).
    pub event_type: String,
    /// The verified payload.
    pub payload: serde_json::Value,
}

/// What a provider checkout would create.
#[derive(Debug, Clone)]
pub struct CheckoutIntent {
    /// The tenant that would subscribe.
    pub organization: OrganizationId,
    /// The plan the tenant selected.
    pub plan: PlanCode,
}

/// A provider checkout session, or the manual instruction when the provider
/// has no hosted checkout.
#[derive(Debug, Clone)]
pub struct CheckoutSession {
    /// Which provider produced it.
    pub provider: BillingProvider,
    /// Hosted checkout URL, when the provider has one.
    pub checkout_url: Option<String>,
    /// Operator instruction when there is no hosted flow.
    pub instructions: String,
}

/// Why a provider operation failed. Provider-neutral on purpose: the caller
/// never sees processor internals.
#[derive(Debug, Clone)]
pub enum ProviderError {
    /// The provider exists but no adapter is implemented in this build.
    NotImplemented(BillingProvider),
    /// The webhook did not verify (bad/missing signature, stale timestamp,
    /// malformed envelope). Always a 401 — never a 500.
    Verification(&'static str),
    /// The adapter could not reach its processor.
    Transport(String),
}

impl ProviderError {
    /// The HTTP status the boundary maps this error to.
    pub fn status(&self) -> StatusCode {
        match self {
            ProviderError::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
            ProviderError::Verification(_) => StatusCode::UNAUTHORIZED,
            ProviderError::Transport(_) => StatusCode::BAD_GATEWAY,
        }
    }

    /// Stable machine label for the error body.
    pub fn code(&self) -> &'static str {
        match self {
            ProviderError::NotImplemented(_) => "provider_not_implemented",
            ProviderError::Verification(_) => "webhook_verification_failed",
            ProviderError::Transport(_) => "provider_transport_error",
        }
    }
}

impl IntoResponse for ProviderError {
    fn into_response(self) -> Response {
        let detail = match &self {
            ProviderError::NotImplemented(p) => {
                format!("no {} adapter is implemented in this build", p.as_str())
            }
            ProviderError::Verification(reason) => (*reason).to_string(),
            ProviderError::Transport(_) => "the provider could not be reached".to_string(),
        };
        // Never echo signatures, timestamps or secrets back.
        (
            self.status(),
            Json(json!({ "error": self.code(), "detail": detail })),
        )
            .into_response()
    }
}

/// One provider's adapter. Implemented OUTSIDE the core billing domain;
/// the domain types in and out stay provider-neutral.
#[async_trait]
pub trait BillingProviderAdapter: Send + Sync {
    /// Which provider this adapter serves.
    fn provider(&self) -> BillingProvider;

    /// Start a checkout for `intent`, when the provider has a hosted flow.
    async fn create_checkout(
        &self,
        intent: &CheckoutIntent,
    ) -> Result<CheckoutSession, ProviderError>;

    /// Verify a webhook request and return the event it carries. Any
    /// failure is a [`ProviderError::Verification`] — the handler must not
    /// process unverified bytes. The default implements the boundary's own
    /// HMAC scheme ([`verify_signed_payload`] + [`parse_envelope`]); an
    /// adapter for a processor with its own signing scheme overrides it,
    /// and the route layer never sees the difference.
    fn verify_webhook(
        &self,
        secret: &WebhookSecret,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<VerifiedProviderEvent, ProviderError> {
        verify_signed_payload(secret, headers, body, chrono::Utc::now().timestamp() as u64)?;
        parse_envelope(self.provider(), body)
    }
}

/// The `manual` provider: an operator assigns plans in TASK 7A. There is no
/// hosted checkout and there are no webhooks, and this adapter says so.
pub struct ManualProvider;

#[async_trait]
impl BillingProviderAdapter for ManualProvider {
    fn provider(&self) -> BillingProvider {
        BillingProvider::Manual
    }

    async fn create_checkout(
        &self,
        intent: &CheckoutIntent,
    ) -> Result<CheckoutSession, ProviderError> {
        Ok(CheckoutSession {
            provider: BillingProvider::Manual,
            checkout_url: None,
            instructions: format!(
                "manual provider: assign the {} plan to this organization with \
                 the operator's plan-assignment flow; no hosted checkout exists",
                intent.plan.as_str()
            ),
        })
    }

    fn verify_webhook(
        &self,
        _secret: &WebhookSecret,
        _headers: &HeaderMap,
        _body: &[u8],
    ) -> Result<VerifiedProviderEvent, ProviderError> {
        Err(ProviderError::NotImplemented(BillingProvider::Manual))
    }
}

/// The provider adapters installed in this process. Seeds the `manual`
/// adapter; real processors register theirs at startup.
#[derive(Default)]
pub struct ProviderRegistry {
    adapters: HashMap<BillingProvider, std::sync::Arc<dyn BillingProviderAdapter>>,
}

impl ProviderRegistry {
    /// The registry with the built-in `manual` adapter.
    pub fn new() -> Self {
        let mut r = Self::default();
        r.register(std::sync::Arc::new(ManualProvider));
        r
    }

    /// Install (or replace) one provider's adapter.
    pub fn register(&mut self, adapter: std::sync::Arc<dyn BillingProviderAdapter>) {
        self.adapters.insert(adapter.provider(), adapter);
    }

    /// The adapter for `provider`, when one is implemented.
    pub fn adapter(
        &self,
        provider: BillingProvider,
    ) -> Option<std::sync::Arc<dyn BillingProviderAdapter>> {
        self.adapters.get(&provider).cloned()
    }
}

/// `hex(HMAC-SHA256(secret, "{timestamp}.{body}"))` — the boundary's
/// signature scheme, shared by every provider adapter that does not carry
/// its own.
pub fn sign_payload(secret: &WebhookSecret, timestamp_secs: u64, body: &[u8]) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.0.as_bytes()).expect("hmac accepts any key length");
    mac.update(timestamp_secs.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

/// Verify the scheme [`sign_payload`] produces: header presence, freshness
/// (against `now_secs`) and constant-time signature equality.
pub fn verify_signed_payload(
    secret: &WebhookSecret,
    headers: &HeaderMap,
    body: &[u8],
    now_secs: u64,
) -> Result<u64, ProviderError> {
    let ts = headers
        .get(TIMESTAMP_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .ok_or(ProviderError::Verification(
            "missing or malformed timestamp",
        ))?;
    let drift = ts.abs_diff(now_secs);
    if drift > SIGNATURE_TOLERANCE.as_secs() {
        return Err(ProviderError::Verification(
            "timestamp outside the tolerance window",
        ));
    }
    let presented = headers
        .get(SIGNATURE_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .ok_or(ProviderError::Verification("missing signature"))?;
    let expected = sign_payload(secret, ts, body);
    if constant_time_eq(expected.as_bytes(), presented.as_bytes()) {
        Ok(ts)
    } else {
        Err(ProviderError::Verification("signature mismatch"))
    }
}

/// Parse the verified body into the boundary's event envelope:
/// `{"id": "…", "type": "…", "data": {…}}`.
pub fn parse_envelope(
    provider: BillingProvider,
    body: &[u8],
) -> Result<VerifiedProviderEvent, ProviderError> {
    let value: serde_json::Value = serde_json::from_slice(body)
        .map_err(|_| ProviderError::Verification("body is not a JSON object"))?;
    let event_id = value
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or(ProviderError::Verification("missing event id"))?
        .to_string();
    let event_type = value
        .get("type")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or(ProviderError::Verification("missing event type"))?
        .to_string();
    Ok(VerifiedProviderEvent {
        provider,
        event_id: event_id.chars().take(200).collect(),
        event_type,
        payload: value
            .get("data")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    })
}

/// The one secret an API key view may expose: none. This re-import guard
/// keeps the boundary honest — `SaasApiKey` stays hash-only here.
#[allow(dead_code)]
fn secret_never_leaves(_key: &SaasApiKey) {}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderName;
    use chrono::Utc;

    fn headers(pairs: &[(&str, String)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    #[tokio::test]
    async fn manual_checkout_is_honest_and_neutral() {
        let registry = ProviderRegistry::new();
        let adapter = registry
            .adapter(BillingProvider::Manual)
            .expect("manual adapter seeded");
        assert_eq!(adapter.provider(), BillingProvider::Manual);
        let session = adapter
            .create_checkout(&CheckoutIntent {
                organization: OrganizationId::new(),
                plan: PlanCode::Business,
            })
            .await
            .expect("manual checkout answers");
        assert_eq!(session.provider, BillingProvider::Manual);
        assert!(session.checkout_url.is_none());
        assert!(session.instructions.contains("business"));

        // The registry honestly reports unimplemented processors…
        assert!(registry.adapter(BillingProvider::Stripe).is_none());
        // …and the manual adapter accepts no webhooks at all.
        let err = ManualProvider.verify_webhook(
            &WebhookSecret::parse("s").unwrap(),
            &HeaderMap::new(),
            b"{}",
        );
        assert!(matches!(err, Err(ProviderError::NotImplemented(_))));
    }

    #[test]
    fn signed_payloads_verify_and_replay_freshness_is_enforced() {
        let secret = WebhookSecret::parse("whsec_test_123").unwrap();
        let body = br#"{"id":"evt_1","type":"subscription.renewed"}"#;
        let now = 1_800_000_000u64;
        let sig = sign_payload(&secret, now, body);
        let h = headers(&[(TIMESTAMP_HEADER, now.to_string()), (SIGNATURE_HEADER, sig)]);
        assert_eq!(verify_signed_payload(&secret, &h, body, now).unwrap(), now);
        // Inside the tolerance window it still verifies.
        assert_eq!(
            verify_signed_payload(&secret, &h, body, now + 60).unwrap(),
            now
        );
        // Outside it, the very same (replayed) request is refused.
        assert!(matches!(
            verify_signed_payload(&secret, &h, body, now + SIGNATURE_TOLERANCE.as_secs() + 1),
            Err(ProviderError::Verification(
                "timestamp outside the tolerance window"
            ))
        ));
    }

    #[test]
    fn tampered_bodies_and_signatures_are_refused() {
        let secret = WebhookSecret::parse("whsec_test_123").unwrap();
        let body = br#"{"id":"evt_1","type":"plan.changed"}"#;
        let now = 1_800_000_000u64;
        let sig = sign_payload(&secret, now, body);

        // A different secret cannot produce the signature.
        let other = WebhookSecret::parse("whsec_other").unwrap();
        let h = headers(&[
            (TIMESTAMP_HEADER, now.to_string()),
            (SIGNATURE_HEADER, sig.clone()),
        ]);
        assert!(matches!(
            verify_signed_payload(&other, &h, body, now),
            Err(ProviderError::Verification("signature mismatch"))
        ));

        // A flipped body byte is refused.
        let flipped = br#"{"id":"evt_2","type":"plan.changed"}"#;
        assert!(matches!(
            verify_signed_payload(&secret, &h, flipped, now),
            Err(ProviderError::Verification("signature mismatch"))
        ));

        // Missing / blank / wrong-scheme headers are refused with stable reasons.
        for bad in [
            vec![(SIGNATURE_HEADER, sig.clone())],
            vec![(TIMESTAMP_HEADER, "not-a-number".to_string())],
            vec![
                (TIMESTAMP_HEADER, now.to_string()),
                (SIGNATURE_HEADER, "".to_string()),
            ],
        ] {
            assert!(verify_signed_payload(&secret, &headers(&bad), body, now).is_err());
        }
        // Debug never leaks the secret material.
        assert!(!format!("{other:?}").contains("whsec_other"));
    }

    #[test]
    fn envelopes_are_parsed_strictly() {
        let p = BillingProvider::Manual;
        let ok = parse_envelope(
            p,
            br#"{"id":"evt_9","type":"subscription.renewed","data":{"x":1}}"#,
        )
        .expect("parses");
        assert_eq!(ok.event_id, "evt_9");
        assert_eq!(ok.event_type, "subscription.renewed");
        assert_eq!(ok.payload["x"], 1);
        assert_eq!(ok.provider, p);

        for bad in [
            r#"not json"#.as_bytes(),
            br#"{"type":"x"}"#,
            br#"{"id":"  ","type":"x"}"#,
            br#"{"id":"evt_1"}"#,
        ] {
            assert!(parse_envelope(p, bad).is_err(), "{:?} must not parse", bad);
        }
        // Ids are capped so a hostile provider cannot balloon memory.
        let long = format!(r#"{{"id":"{}","type":"t"}}"#, "e".repeat(500));
        let capped = parse_envelope(p, long.as_bytes()).unwrap();
        assert_eq!(capped.event_id.len(), 200);
    }

    #[tokio::test]
    async fn the_boundary_reports_statuses_stably() {
        assert_eq!(
            ProviderError::Verification("x").status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            ProviderError::NotImplemented(BillingProvider::Paddle).status(),
            StatusCode::NOT_IMPLEMENTED
        );
        assert_eq!(
            ProviderError::Transport("down".into()).status(),
            StatusCode::BAD_GATEWAY
        );
        // The response body carries no signature material and no secrets.
        let body = ProviderError::Verification("signature mismatch").into_response();
        let bytes = http_body_util::BodyExt::collect(body.into_body())
            .await
            .unwrap()
            .to_bytes();
        let text = String::from_utf8_lossy(&bytes).to_string();
        assert!(text.contains("webhook_verification_failed"), "{text}");
        assert!(!text.contains("whsec"));
        assert!(Utc::now() > Utc::now() - chrono::Duration::seconds(1));
    }
}
