//! LIVE Stripe HTTP boundary — exercised against **Stripe's own `stripe-mock`
//! server**, over real HTTP, with the adapter's real request building, real
//! `Idempotency-Key` propagation, real JSON decoding and real webhook HMAC
//! verification.
//!
//! `#[ignore]`d by default: it needs a running `stripe-mock` (a Stripe-published
//! binary, not something written here) and is pointed at it with
//! `STRIPE_MOCK_URL`:
//!
//! ```text
//! stripe-mock -http-port 12111 &
//! STRIPE_MOCK_URL=http://127.0.0.1:12111 \
//!   cargo test -p sniper-suite --test live_stripe_mock -- --ignored --nocapture --test-threads=1
//! ```
//!
//! What this proves: the adapter speaks the Checkout Sessions API shape Stripe
//! documents (`POST /v1/checkout/sessions`, bearer auth, form body), survives a
//! real success response, and verifies a real `Stripe-Signature` HMAC — while
//! rejecting a tampered one.
//!
//! What it does NOT prove, and does not claim: a live Stripe account, real
//! money, real webhook delivery, or real price/plan ids. Those need Stripe
//! credentials this environment does not have; `LIVE_BILLING=1` against
//! `api.stripe.com` remains an operator step.

use bot_core::billing::PlanCode;
use bot_core::tenant::OrganizationId;
use sniper_suite::billing::stripe_adapter::{StripeAdapter, StripeCheckoutIntent, StripeError};

/// Stripe-shaped test key. stripe-mock accepts any bearer token; this is not a
/// real credential and cannot reach a live account.
const MOCK_KEY: &str = "sk_test_snipermock0000000000000000";

fn mock_base() -> Option<String> {
    match std::env::var("STRIPE_MOCK_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url.trim().trim_end_matches('/').to_string()),
        _ => {
            eprintln!(
                "SKIPPED: set STRIPE_MOCK_URL (e.g. http://127.0.0.1:12111) to run this test"
            );
            None
        }
    }
}

fn intent() -> StripeCheckoutIntent {
    StripeCheckoutIntent {
        organization: OrganizationId::new(),
        plan: PlanCode::Pro,
        idempotency_key: "sniper-live-stripe-mock-1".to_string(),
        success_url: Some("https://sniper.example.test/billing/success".to_string()),
        cancel_url: Some("https://sniper.example.test/billing/cancel".to_string()),
    }
}

/// The opt-in gate must stay shut unless the operator opens it — even when a
/// perfectly reachable provider is configured.
#[tokio::test]
#[ignore = "requires a running stripe-mock (STRIPE_MOCK_URL)"]
async fn live_stripe_refuses_without_live_billing_opt_in() {
    let Some(base) = mock_base() else { return };
    std::env::remove_var("LIVE_BILLING");
    let adapter = StripeAdapter::with_base_for_test(MOCK_KEY, base);
    let err = adapter
        .create_checkout(&intent())
        .await
        .expect_err("checkout must refuse unless LIVE_BILLING=1");
    match err {
        StripeError::Transport(msg) => assert!(
            msg.contains("LIVE_BILLING"),
            "the refusal must name the missing opt-in, got: {msg}"
        ),
        other => panic!("unexpected error kind: {other:?}"),
    }
}

/// A real `POST /v1/checkout/sessions` against stripe-mock, decoded by the
/// adapter's own parser.
#[tokio::test]
#[ignore = "requires a running stripe-mock (STRIPE_MOCK_URL)"]
async fn live_stripe_creates_a_checkout_session_over_http() {
    let Some(base) = mock_base() else { return };
    std::env::set_var("LIVE_BILLING", "1");
    let adapter = StripeAdapter::with_base_for_test(MOCK_KEY, base.clone());

    let session = adapter
        .create_checkout(&intent())
        .await
        .expect("stripe-mock must accept the checkout request");

    assert!(
        !session.session_id.trim().is_empty(),
        "a decoded session must carry a provider id"
    );
    assert!(
        session.checkout_url.starts_with("http"),
        "checkout_url must be a URL, got {}",
        session.checkout_url
    );
    assert!(
        session.instructions.contains(&session.session_id),
        "instructions must name the real provider session"
    );
    eprintln!(
        "LIVE stripe-mock checkout ok: session_id={} url={}",
        session.session_id, session.checkout_url
    );
    std::env::remove_var("LIVE_BILLING");
}

/// Idempotency-Key must reach the provider; the same key twice must not error
/// out (Stripe deduplicates), and a second, different key must still succeed.
#[tokio::test]
#[ignore = "requires a running stripe-mock (STRIPE_MOCK_URL)"]
async fn live_stripe_propagates_idempotency_key() {
    let Some(base) = mock_base() else { return };
    std::env::set_var("LIVE_BILLING", "1");
    let adapter = StripeAdapter::with_base_for_test(MOCK_KEY, base);

    let mut first = intent();
    first.idempotency_key = "sniper-live-idem-abc".to_string();
    let session_a = adapter.create_checkout(&first).await.expect("first");
    let session_b = adapter.create_checkout(&first).await.expect("replay");
    // stripe-mock is stateless about idempotency, so ids may repeat; what is
    // asserted is that a replayed key is not an error and yields a usable
    // session both times.
    assert!(!session_a.session_id.is_empty() && !session_b.session_id.is_empty());

    let mut second = intent();
    second.idempotency_key = "sniper-live-idem-xyz".to_string();
    let session_c = adapter.create_checkout(&second).await.expect("new key");
    assert!(!session_c.session_id.is_empty());
    eprintln!(
        "LIVE stripe-mock idempotency ok: {} / {} / {}",
        session_a.session_id, session_b.session_id, session_c.session_id
    );
    std::env::remove_var("LIVE_BILLING");
}

/// Webhook verification with a real HMAC over `{t}.{body}`, plus the rejection
/// paths that must never be silently accepted.
#[tokio::test]
#[ignore = "requires a running stripe-mock (STRIPE_MOCK_URL) (only for the shared setup)"]
async fn live_stripe_webhook_signature_round_trip() {
    use axum::http::{HeaderMap, HeaderValue};
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let Some(_base) = mock_base() else { return };
    let adapter = StripeAdapter::with_base_for_test(MOCK_KEY, "http://127.0.0.1:1");
    let secret = "whsec_sniper_live_mock_secret";
    let body = br#"{"id":"evt_live_mock_1","type":"checkout.session.completed","data":{"object":{"id":"cs_test_live_mock_1","payment_status":"paid"}}}"#;
    // Real current unix time: the verifier enforces a tolerance window, so a
    // fixed historical timestamp is (correctly) rejected as stale.
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after the epoch")
        .as_secs();

    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    let signature = hex::encode(mac.finalize().into_bytes());

    let mut headers = HeaderMap::new();
    headers.insert(
        "stripe-signature",
        HeaderValue::from_str(&format!("t={timestamp},v1={signature}")).expect("header"),
    );

    // 1. The genuine signature verifies and yields the parsed event.
    let event = adapter
        .verify_webhook(secret, &headers, body)
        .expect("a correctly signed webhook must verify");
    assert_eq!(
        event.get("type").and_then(|v| v.as_str()),
        Some("checkout.session.completed")
    );

    // 2. A tampered body must fail.
    let mut tampered = body.to_vec();
    tampered.extend_from_slice(b" ");
    assert!(
        adapter.verify_webhook(secret, &headers, &tampered).is_err(),
        "a tampered body must not verify"
    );

    // 3. A wrong secret must fail.
    assert!(
        adapter
            .verify_webhook("whsec_a_different_secret", &headers, body)
            .is_err(),
        "a signature from another secret must not verify"
    );

    // 4. A missing signature header must fail (no implicit trust).
    assert!(
        adapter
            .verify_webhook(secret, &HeaderMap::new(), body)
            .is_err(),
        "an unsigned webhook must not verify"
    );

    // 5. A stale timestamp (replay) must fail even when the HMAC is correct.
    let stale = 1_600_000_000u64;
    let mut stale_mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key");
    stale_mac.update(stale.to_string().as_bytes());
    stale_mac.update(b".");
    stale_mac.update(body);
    let stale_signature = hex::encode(stale_mac.finalize().into_bytes());
    let mut stale_headers = HeaderMap::new();
    stale_headers.insert(
        "stripe-signature",
        HeaderValue::from_str(&format!("t={stale},v1={stale_signature}")).expect("header"),
    );
    assert!(
        adapter
            .verify_webhook(secret, &stale_headers, body)
            .is_err(),
        "a correctly signed but stale (replayed) webhook must be rejected"
    );

    eprintln!(
        "LIVE stripe webhook HMAC: valid verified; tampered, wrong-secret, unsigned and \
         stale-replay all rejected"
    );
}
