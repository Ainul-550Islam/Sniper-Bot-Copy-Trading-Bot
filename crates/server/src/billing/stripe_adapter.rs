//! Stripe billing adapter — real API boundary (GAP-04).
//!
//! Typed configuration errors and live-network gating. No silent fallback to manual.
//! Secrets are never logged or returned.

use bot_core::billing::PlanCode;
use bot_core::tenant::OrganizationId;

/// Stripe API base — production. Tests can inject a custom base.
const STRIPE_API_BASE: &str = "https://api.stripe.com";
const STRIPE_CHECKOUT_PATH: &str = "/v1/checkout/sessions";

/// Bounded provider request timeout. A hung or slow provider must surface as a
/// classified transport failure instead of blocking a checkout task forever.
pub const STRIPE_HTTP_TIMEOUT_SECS: u64 = 15;

fn stripe_http_client(timeout_secs: u64) -> Result<reqwest::Client, StripeError> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
        .map_err(|e| StripeError::Transport(format!("stripe http client: {e}")))
}

/// Typed provider error — distinguishes not-configured from transport failures.
#[derive(Debug, Clone)]
pub enum StripeError {
    NotConfigured(String),
    Transport(String),
    Verification(String),
}

impl std::fmt::Display for StripeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StripeError::NotConfigured(msg) => write!(f, "stripe not configured: {msg}"),
            StripeError::Transport(msg) => write!(f, "stripe transport: {msg}"),
            StripeError::Verification(msg) => write!(f, "stripe verification: {msg}"),
        }
    }
}
impl std::error::Error for StripeError {}

/// Checkout intent for Stripe — organization + plan, no client-supplied amount.
#[derive(Debug, Clone)]
pub struct StripeCheckoutIntent {
    pub organization: OrganizationId,
    pub plan: PlanCode,
    pub idempotency_key: String,
    pub success_url: Option<String>,
    pub cancel_url: Option<String>,
}

/// Checkout session result from Stripe.
#[derive(Debug, Clone)]
pub struct StripeCheckoutSession {
    pub checkout_url: String,
    pub session_id: String,
    pub instructions: String,
}

/// Stripe adapter. Holds the API key reference (never the raw key in logs) and an HTTP client.
pub struct StripeAdapter {
    api_key: String,
    client: reqwest::Client,
    base_url: String,
}

impl StripeAdapter {
    /// Create from environment. Returns typed NotConfigured error when STRIPE_API_KEY is missing/empty — no silent fallback.
    pub fn from_env() -> Result<Self, StripeError> {
        let api_key = std::env::var("STRIPE_API_KEY")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| {
                StripeError::NotConfigured(
                    "missing STRIPE_API_KEY (set STRIPE_API_KEY in environment; no silent fallback to manual)".into(),
                )
            })?;
        if api_key.len() < 8 {
            return Err(StripeError::NotConfigured(
                "STRIPE_API_KEY too short".into(),
            ));
        }
        Ok(Self {
            api_key,
            client: stripe_http_client(STRIPE_HTTP_TIMEOUT_SECS)?,
            base_url: STRIPE_API_BASE.to_string(),
        })
    }

    /// For tests: inject a custom base URL (e.g., mock server) and key.
    pub fn with_base_for_test(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            client: stripe_http_client(STRIPE_HTTP_TIMEOUT_SECS)
                .unwrap_or_else(|_| reqwest::Client::new()),
            base_url: base_url.into(),
        }
    }

    /// For tests: custom base URL with an explicit (short) request timeout,
    /// used to prove that a hung provider fails closed within a bounded time.
    pub fn with_base_and_timeout_for_test(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        timeout_secs: u64,
    ) -> Result<Self, StripeError> {
        Ok(Self {
            api_key: api_key.into(),
            client: stripe_http_client(timeout_secs)?,
            base_url: base_url.into(),
        })
    }

    fn is_live_enabled() -> bool {
        std::env::var("LIVE_BILLING")
            .map(|v| v == "1")
            .unwrap_or(false)
    }

    /// Create a Stripe checkout session via real API. Gated by LIVE_BILLING=1.
    /// Propagates idempotency_key via Idempotency-Key header (Stripe supports it).
    pub async fn create_checkout(
        &self,
        intent: &StripeCheckoutIntent,
    ) -> Result<StripeCheckoutSession, StripeError> {
        if !Self::is_live_enabled() {
            return Err(StripeError::Transport(
                "LIVE_BILLING != 1 — live Stripe checkout NOT_RUN (explicit opt-in required)"
                    .into(),
            ));
        }
        let success_url = intent
            .success_url
            .clone()
            .unwrap_or_else(|| format!("https://example.com/success?org={}", intent.organization));
        let cancel_url = intent
            .cancel_url
            .clone()
            .unwrap_or_else(|| format!("https://example.com/cancel?org={}", intent.organization));
        let plan = intent.plan.as_str();

        let params = [
            ("success_url", success_url.as_str()),
            ("cancel_url", cancel_url.as_str()),
            ("mode", "subscription"),
            ("client_reference_id", &intent.organization.to_string()),
        ];

        let url = format!("{}{}", self.base_url, STRIPE_CHECKOUT_PATH);
        let mut req = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .form(&params);
        // Provider-side idempotency: Stripe respects Idempotency-Key header
        if !intent.idempotency_key.trim().is_empty() {
            req = req.header("Idempotency-Key", intent.idempotency_key.clone());
        }
        let resp = req
            .send()
            .await
            .map_err(|e| StripeError::Transport(format!("stripe transport error: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(StripeError::Transport(format!(
                "stripe checkout failed: status {} body_len {} plan {}",
                status.as_u16(),
                body.len(),
                plan
            )));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| StripeError::Transport(format!("stripe decode failed: {e}")))?;

        let checkout_url = json
            .get("url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        // A provider response without a session id is not usable: the id is the
        // durable provider identity and is covered by
        // UNIQUE (provider, provider_session_id). An empty id would collide
        // across checkouts, so it must fail closed instead of being stored.
        let session_id = json
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                StripeError::Transport(
                    "stripe checkout succeeded but returned no session id".into(),
                )
            })?;

        let url = checkout_url.ok_or_else(|| {
            StripeError::Transport("stripe checkout succeeded but returned no url".into())
        })?;

        Ok(StripeCheckoutSession {
            checkout_url: url.clone(),
            session_id: session_id.clone(),
            instructions: format!(
                "stripe checkout for {} plan — session {} — redirect to hosted url",
                plan, session_id
            ),
        })
    }

    /// Verify Stripe webhook signature: supports Stripe-Signature header (t=...,v1=...) and generic fallback.
    pub fn verify_webhook(
        &self,
        secret: &str,
        headers: &axum::http::HeaderMap,
        body: &[u8],
    ) -> Result<serde_json::Value, StripeError> {
        // Try Stripe-Signature header first
        if let Some(sig_header) = headers
            .get("stripe-signature")
            .or_else(|| headers.get("Stripe-Signature"))
            .and_then(|v| v.to_str().ok())
        {
            let timestamp = sig_header
                .split(',')
                .find(|p| p.starts_with("t="))
                .and_then(|p| p[2..].parse::<u64>().ok())
                .ok_or_else(|| StripeError::Verification("missing stripe timestamp".into()))?;
            let v1 = sig_header
                .split(',')
                .find(|p| p.starts_with("v1="))
                .map(|p| &p[3..])
                .ok_or_else(|| StripeError::Verification("missing stripe v1 signature".into()))?;
            // Compute expected HMAC: hex(HMAC-SHA256(secret, "timestamp.body"))
            use hmac::{Hmac, Mac};
            use sha2::Sha256;
            let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
                .map_err(|e| StripeError::Verification(format!("hmac init: {e}")))?;
            mac.update(timestamp.to_string().as_bytes());
            mac.update(b".");
            mac.update(body);
            let expected = hex::encode(mac.finalize().into_bytes());
            if !bot_core::session::token::constant_time_eq(expected.as_bytes(), v1.as_bytes()) {
                return Err(StripeError::Verification(
                    "stripe signature mismatch".into(),
                ));
            }
            let now = chrono::Utc::now().timestamp() as u64;
            if timestamp.abs_diff(now) > 300 {
                return Err(StripeError::Verification(
                    "stripe timestamp outside tolerance window".into(),
                ));
            }
            return serde_json::from_slice(body)
                .map_err(|e| StripeError::Verification(format!("body is not json: {e}")));
        }
        // Generic fallback: expect x-provider-timestamp / x-provider-signature
        let ts = headers
            .get("x-provider-timestamp")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .ok_or_else(|| StripeError::Verification("missing x-provider-timestamp".into()))?;
        let sig = headers
            .get("x-provider-signature")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| StripeError::Verification("missing x-provider-signature".into()))?;
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
            .map_err(|e| StripeError::Verification(format!("hmac: {e}")))?;
        mac.update(ts.to_string().as_bytes());
        mac.update(b".");
        mac.update(body);
        let expected = hex::encode(mac.finalize().into_bytes());
        if !bot_core::session::token::constant_time_eq(expected.as_bytes(), sig.as_bytes()) {
            return Err(StripeError::Verification("signature mismatch".into()));
        }
        let now = chrono::Utc::now().timestamp() as u64;
        if ts.abs_diff(now) > 300 {
            return Err(StripeError::Verification("timestamp outside window".into()));
        }
        serde_json::from_slice(body)
            .map_err(|e| StripeError::Verification(format!("body is not json: {e}")))
    }
}

impl std::fmt::Debug for StripeAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StripeAdapter")
            .field("base_url", &self.base_url)
            .field("has_key", &!self.api_key.is_empty())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    #[test]
    fn stripe_not_configured_returns_typed_error() {
        let prev = std::env::var("STRIPE_API_KEY").ok();
        std::env::remove_var("STRIPE_API_KEY");
        let err = StripeAdapter::from_env();
        assert!(matches!(err, Err(StripeError::NotConfigured(_))));
        if let Err(StripeError::NotConfigured(msg)) = err {
            assert!(msg.contains("STRIPE_API_KEY"), "{msg}");
            assert!(msg.contains("no silent fallback"), "{msg}");
        }
        if let Some(v) = prev {
            std::env::set_var("STRIPE_API_KEY", v);
        }
    }

    #[tokio::test]
    async fn stripe_checkout_requires_live_flag() {
        let prev_key = std::env::var("STRIPE_API_KEY").ok();
        let prev_live = std::env::var("LIVE_BILLING").ok();
        std::env::set_var("STRIPE_API_KEY", "sk_test_12345678");
        std::env::remove_var("LIVE_BILLING");
        let adapter = StripeAdapter::from_env().expect("configured");
        let intent = StripeCheckoutIntent {
            organization: OrganizationId::new(),
            plan: PlanCode::Pro,
            idempotency_key: format!("test-{}", uuid::Uuid::new_v4()),
            success_url: None,
            cancel_url: None,
        };
        let res = adapter.create_checkout(&intent).await;
        assert!(matches!(res, Err(StripeError::Transport(_))));
        if let Some(v) = prev_key {
            std::env::set_var("STRIPE_API_KEY", v);
        } else {
            std::env::remove_var("STRIPE_API_KEY");
        }
        if let Some(v) = prev_live {
            std::env::set_var("LIVE_BILLING", v);
        } else {
            std::env::remove_var("LIVE_BILLING");
        }
    }

    #[test]
    #[ignore]
    fn stripe_live_checkout_ignored_without_live_billing() {
        let live = std::env::var("LIVE_BILLING").unwrap_or_default() == "1"
            && std::env::var("STRIPE_API_KEY")
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
        assert!(
            live,
            "LIVE_BILLING=1 and STRIPE_API_KEY required for live test; skipping"
        );
    }

    #[test]
    fn stripe_debug_does_not_leak_key() {
        let adapter =
            StripeAdapter::with_base_for_test("sk_live_very_secret_12345678", STRIPE_API_BASE);
        let dbg = format!("{:?}", adapter);
        assert!(!dbg.contains("sk_live_very_secret"));
        assert!(dbg.contains("has_key"));
    }

    // Helper to spawn a mock Stripe server returning status and body
    async fn mock_stripe_server(status: u16, body: serde_json::Value) -> String {
        use axum::{routing::post, Json, Router};
        let app = Router::new().route(
            STRIPE_CHECKOUT_PATH,
            post(
                move |_headers: axum::http::HeaderMap, _body: String| async move {
                    let s = axum::http::StatusCode::from_u16(status)
                        .unwrap_or(axum::http::StatusCode::OK);
                    (s, Json(body.clone())).into_response()
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{}", addr)
    }

    #[tokio::test]
    async fn stripe_handles_401_without_leaking_key() {
        let prev_live = std::env::var("LIVE_BILLING").ok();
        std::env::set_var("LIVE_BILLING", "1");
        let base = mock_stripe_server(401, serde_json::json!({"error": "unauthorized"})).await;
        let adapter = StripeAdapter::with_base_for_test("sk_live_secret_12345678", base);
        let intent = StripeCheckoutIntent {
            organization: OrganizationId::new(),
            plan: PlanCode::Pro,
            idempotency_key: format!("idem-{}", uuid::Uuid::new_v4()),
            success_url: None,
            cancel_url: None,
        };
        let res = adapter.create_checkout(&intent).await;
        assert!(matches!(res, Err(StripeError::Transport(_))));
        if let Err(StripeError::Transport(msg)) = res {
            assert!(msg.contains("401"), "{msg}");
            assert!(!msg.contains("sk_live_secret"), "must not leak key");
            assert!(msg.contains("stripe checkout failed"), "{msg}");
        }
        if let Some(v) = prev_live {
            std::env::set_var("LIVE_BILLING", v);
        } else {
            std::env::remove_var("LIVE_BILLING");
        }
    }

    #[tokio::test]
    async fn stripe_handles_403_without_leaking_key() {
        let prev_live = std::env::var("LIVE_BILLING").ok();
        std::env::set_var("LIVE_BILLING", "1");
        let base = mock_stripe_server(403, serde_json::json!({"error": "forbidden"})).await;
        let adapter = StripeAdapter::with_base_for_test("sk_live_secret_12345678", base);
        let intent = StripeCheckoutIntent {
            organization: OrganizationId::new(),
            plan: PlanCode::Pro,
            idempotency_key: format!("idem-{}", uuid::Uuid::new_v4()),
            success_url: None,
            cancel_url: None,
        };
        let res = adapter.create_checkout(&intent).await;
        assert!(matches!(res, Err(StripeError::Transport(_))));
        if let Err(StripeError::Transport(msg)) = res {
            assert!(msg.contains("403"), "{msg}");
            assert!(!msg.contains("sk_live_secret"));
        }
        if let Some(v) = prev_live {
            std::env::set_var("LIVE_BILLING", v);
        } else {
            std::env::remove_var("LIVE_BILLING");
        }
    }

    #[tokio::test]
    async fn stripe_handles_500_without_leaking_key() {
        let prev_live = std::env::var("LIVE_BILLING").ok();
        std::env::set_var("LIVE_BILLING", "1");
        let base = mock_stripe_server(500, serde_json::json!({"error": "internal"})).await;
        let adapter = StripeAdapter::with_base_for_test("sk_live_secret_12345678", base);
        let intent = StripeCheckoutIntent {
            organization: OrganizationId::new(),
            plan: PlanCode::Starter,
            idempotency_key: format!("idem-{}", uuid::Uuid::new_v4()),
            success_url: None,
            cancel_url: None,
        };
        let res = adapter.create_checkout(&intent).await;
        assert!(matches!(res, Err(StripeError::Transport(_))));
        if let Err(StripeError::Transport(msg)) = res {
            assert!(msg.contains("500"));
            assert!(!msg.contains("sk_live_secret"));
        }
        if let Some(v) = prev_live {
            std::env::set_var("LIVE_BILLING", v);
        } else {
            std::env::remove_var("LIVE_BILLING");
        }
    }

    #[tokio::test]
    async fn stripe_handles_malformed_json() {
        let prev_live = std::env::var("LIVE_BILLING").ok();
        std::env::set_var("LIVE_BILLING", "1");
        // Return 200 with invalid JSON body (plain text)
        use axum::{routing::post, Router};
        let app = Router::new().route(
            STRIPE_CHECKOUT_PATH,
            post(|| async { (axum::http::StatusCode::OK, "not json") }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let base = format!("http://{}", addr);
        let adapter = StripeAdapter::with_base_for_test("sk_test_12345678", base);
        let intent = StripeCheckoutIntent {
            organization: OrganizationId::new(),
            plan: PlanCode::Pro,
            idempotency_key: format!("idem-{}", uuid::Uuid::new_v4()),
            success_url: None,
            cancel_url: None,
        };
        let res = adapter.create_checkout(&intent).await;
        assert!(matches!(res, Err(StripeError::Transport(_))));
        if let Err(StripeError::Transport(msg)) = res {
            assert!(msg.contains("decode") || msg.contains("stripe"), "{msg}");
        }
        if let Some(v) = prev_live {
            std::env::set_var("LIVE_BILLING", v);
        } else {
            std::env::remove_var("LIVE_BILLING");
        }
    }

    #[tokio::test]
    async fn stripe_handles_missing_url() {
        let prev_live = std::env::var("LIVE_BILLING").ok();
        std::env::set_var("LIVE_BILLING", "1");
        let base = mock_stripe_server(
            200,
            serde_json::json!({"id": "cs_test_123", "object": "checkout.session"}),
        )
        .await;
        let adapter = StripeAdapter::with_base_for_test("sk_test_12345678", base);
        let intent = StripeCheckoutIntent {
            organization: OrganizationId::new(),
            plan: PlanCode::Pro,
            idempotency_key: format!("idem-{}", uuid::Uuid::new_v4()),
            success_url: None,
            cancel_url: None,
        };
        let res = adapter.create_checkout(&intent).await;
        assert!(matches!(res, Err(StripeError::Transport(_))));
        if let Err(StripeError::Transport(msg)) = res {
            assert!(msg.contains("no url"), "{msg}");
        }
        if let Some(v) = prev_live {
            std::env::set_var("LIVE_BILLING", v);
        } else {
            std::env::remove_var("LIVE_BILLING");
        }
    }

    #[tokio::test]
    async fn stripe_propagates_idempotency_key() {
        let prev_live = std::env::var("LIVE_BILLING").ok();
        std::env::set_var("LIVE_BILLING", "1");
        use axum::{http::HeaderMap, routing::post, Json, Router};
        use std::sync::{Arc, Mutex};
        let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let captured_clone = captured.clone();
        let app = Router::new().route(
            STRIPE_CHECKOUT_PATH,
            post(move |headers: HeaderMap, _body: String| {
                let captured = captured_clone.clone();
                async move {
                    if let Some(v) = headers.get("Idempotency-Key").and_then(|h| h.to_str().ok()) {
                        *captured.lock().unwrap() = Some(v.to_string());
                    }
                    let body = serde_json::json!({"id": "cs_test_123", "url": "https://checkout.stripe.com/pay/cs_test_123"});
                    (axum::http::StatusCode::OK, Json(body)).into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let base = format!("http://{}", addr);
        let adapter = StripeAdapter::with_base_for_test("sk_test_12345678", base);
        let key = format!("idem-check-{}", uuid::Uuid::new_v4());
        let intent = StripeCheckoutIntent {
            organization: OrganizationId::new(),
            plan: PlanCode::Pro,
            idempotency_key: key.clone(),
            success_url: None,
            cancel_url: None,
        };
        let res = adapter
            .create_checkout(&intent)
            .await
            .expect("should succeed");
        assert_eq!(res.session_id, "cs_test_123");
        assert_eq!(*captured.lock().unwrap(), Some(key));
        if let Some(v) = prev_live {
            std::env::set_var("LIVE_BILLING", v);
        } else {
            std::env::remove_var("LIVE_BILLING");
        }
    }

    #[tokio::test]
    async fn stripe_missing_session_id_is_an_error() {
        let prev_live = std::env::var("LIVE_BILLING").ok();
        std::env::set_var("LIVE_BILLING", "1");
        // 200 with a URL but no id: an unusable provider identity must not be
        // stored (it would collide under UNIQUE (provider, provider_session_id)).
        let base = mock_stripe_server(
            200,
            serde_json::json!({"url": "https://checkout.stripe.com/pay/cs_test"}),
        )
        .await;
        let adapter = StripeAdapter::with_base_for_test("sk_test_12345678", base);
        let intent = StripeCheckoutIntent {
            organization: OrganizationId::new(),
            plan: PlanCode::Pro,
            idempotency_key: format!("idem-{}", uuid::Uuid::new_v4()),
            success_url: None,
            cancel_url: None,
        };
        let res = adapter.create_checkout(&intent).await;
        assert!(matches!(res, Err(StripeError::Transport(_))));
        if let Err(StripeError::Transport(msg)) = res {
            assert!(msg.contains("no session id"), "{msg}");
        }
        if let Some(v) = prev_live {
            std::env::set_var("LIVE_BILLING", v);
        } else {
            std::env::remove_var("LIVE_BILLING");
        }
    }

    #[tokio::test]
    async fn stripe_timeout_is_a_bounded_classified_error() {
        let prev_live = std::env::var("LIVE_BILLING").ok();
        std::env::set_var("LIVE_BILLING", "1");
        // Provider that never answers within the client timeout.
        use axum::{routing::post, Router};
        let app = Router::new().route(
            STRIPE_CHECKOUT_PATH,
            post(|| async {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                (axum::http::StatusCode::OK, "{}")
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let base = format!("http://{}", addr);
        let adapter =
            StripeAdapter::with_base_and_timeout_for_test("sk_test_12345678", base, 1).unwrap();
        let started = std::time::Instant::now();
        let intent = StripeCheckoutIntent {
            organization: OrganizationId::new(),
            plan: PlanCode::Pro,
            idempotency_key: format!("idem-{}", uuid::Uuid::new_v4()),
            success_url: None,
            cancel_url: None,
        };
        let res = adapter.create_checkout(&intent).await;
        assert!(
            matches!(res, Err(StripeError::Transport(_))),
            "a hung provider must fail closed"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "the failure must be bounded by the client timeout, took {:?}",
            started.elapsed()
        );
        if let Some(v) = prev_live {
            std::env::set_var("LIVE_BILLING", v);
        } else {
            std::env::remove_var("LIVE_BILLING");
        }
    }
}
