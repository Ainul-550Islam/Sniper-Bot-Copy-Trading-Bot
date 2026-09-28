//! Paddle billing adapter — real API boundary (GAP-04).
//!
//! Typed config error, live gating, no silent fallback. Secrets never logged.

use bot_core::billing::PlanCode;
use bot_core::tenant::OrganizationId;

const PADDLE_API_BASE: &str = "https://api.paddle.com";
const PADDLE_CHECKOUT_PATH: &str = "/3.2/checkout/sessions";

/// Bounded provider request timeout. A hung or slow provider must surface as a
/// classified transport failure instead of blocking a checkout task forever.
pub const PADDLE_HTTP_TIMEOUT_SECS: u64 = 15;

fn paddle_http_client(timeout_secs: u64) -> Result<reqwest::Client, PaddleError> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
        .map_err(|e| PaddleError::Transport(format!("paddle http client: {e}")))
}

#[derive(Debug, Clone)]
pub enum PaddleError {
    NotConfigured(String),
    Transport(String),
    Verification(String),
}
impl std::fmt::Display for PaddleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PaddleError::NotConfigured(m) => write!(f, "paddle not configured: {m}"),
            PaddleError::Transport(m) => write!(f, "paddle transport: {m}"),
            PaddleError::Verification(m) => write!(f, "paddle verification: {m}"),
        }
    }
}
impl std::error::Error for PaddleError {}

#[derive(Debug, Clone)]
pub struct PaddleCheckoutIntent {
    pub organization: OrganizationId,
    pub plan: PlanCode,
    pub idempotency_key: String,
    pub success_url: Option<String>,
    pub cancel_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PaddleCheckoutSession {
    pub checkout_url: String,
    pub session_id: String,
    pub instructions: String,
}

pub struct PaddleAdapter {
    api_key: String,
    client: reqwest::Client,
    base_url: String,
}

impl PaddleAdapter {
    pub fn from_env() -> Result<Self, PaddleError> {
        let api_key = std::env::var("PADDLE_API_KEY")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| {
                PaddleError::NotConfigured(
                    "missing PADDLE_API_KEY (set PADDLE_API_KEY in environment; no silent fallback to manual)".into(),
                )
            })?;
        if api_key.len() < 8 {
            return Err(PaddleError::NotConfigured(
                "PADDLE_API_KEY too short".into(),
            ));
        }
        Ok(Self {
            api_key,
            client: paddle_http_client(PADDLE_HTTP_TIMEOUT_SECS)?,
            base_url: PADDLE_API_BASE.to_string(),
        })
    }

    pub fn with_base_for_test(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            client: paddle_http_client(PADDLE_HTTP_TIMEOUT_SECS)
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
    ) -> Result<Self, PaddleError> {
        Ok(Self {
            api_key: api_key.into(),
            client: paddle_http_client(timeout_secs)?,
            base_url: base_url.into(),
        })
    }

    fn is_live_enabled() -> bool {
        std::env::var("LIVE_BILLING")
            .map(|v| v == "1")
            .unwrap_or(false)
    }

    pub async fn create_checkout(
        &self,
        intent: &PaddleCheckoutIntent,
    ) -> Result<PaddleCheckoutSession, PaddleError> {
        if !Self::is_live_enabled() {
            return Err(PaddleError::Transport(
                "LIVE_BILLING != 1 — live Paddle checkout NOT_RUN (explicit opt-in required)"
                    .into(),
            ));
        }
        let body = serde_json::json!({
            "customer": { "reference": intent.organization.to_string() },
            "items": [{ "price_id": format!("paddle_price_{}", intent.plan.as_str()), "quantity": 1 }],
            "custom_data": { "organization_id": intent.organization.to_string(), "plan": intent.plan.as_str(), "idempotency_key": intent.idempotency_key.clone() },
            "success_url": intent.success_url.clone().unwrap_or_else(|| format!("https://example.com/success?org={}", intent.organization)),
            "cancel_url": intent.cancel_url.clone().unwrap_or_else(|| format!("https://example.com/cancel?org={}", intent.organization))
        });
        let url = format!("{}{}", self.base_url, PADDLE_CHECKOUT_PATH);
        let mut req = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body);
        // Provider-side idempotency: Paddle supports Idempotency-Key header (documented for idempotent requests)
        if !intent.idempotency_key.trim().is_empty() {
            req = req.header("Idempotency-Key", intent.idempotency_key.clone());
        }
        let resp = req
            .send()
            .await
            .map_err(|e| PaddleError::Transport(format!("paddle transport error: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let bytes = resp.bytes().await.unwrap_or_default();
            return Err(PaddleError::Transport(format!(
                "paddle checkout failed: status {} body_len {} plan {}",
                status.as_u16(),
                bytes.len(),
                intent.plan.as_str()
            )));
        }
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| PaddleError::Transport(format!("paddle decode failed: {e}")))?;
        let checkout_url = json
            .get("data")
            .and_then(|d| d.get("url"))
            .or_else(|| json.get("url"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        // A provider response without a session id is not usable: the id is the
        // durable provider identity and is covered by
        // UNIQUE (provider, provider_session_id). A fabricated placeholder would
        // collide across checkouts, so it must fail closed instead.
        let session_id = json
            .get("data")
            .and_then(|d| d.get("id"))
            .or_else(|| json.get("id"))
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                PaddleError::Transport(
                    "paddle checkout succeeded but returned no session id".into(),
                )
            })?;
        let url = checkout_url.ok_or_else(|| {
            PaddleError::Transport("paddle checkout succeeded but returned no url".into())
        })?;
        Ok(PaddleCheckoutSession {
            checkout_url: url.clone(),
            session_id: session_id.clone(),
            instructions: format!(
                "paddle checkout for {} plan — session {} — redirect to hosted url",
                intent.plan.as_str(),
                session_id
            ),
        })
    }

    pub fn verify_webhook(
        &self,
        secret: &str,
        headers: &axum::http::HeaderMap,
        body: &[u8],
    ) -> Result<serde_json::Value, PaddleError> {
        if let Some(sig_header) = headers
            .get("paddle-signature")
            .or_else(|| headers.get("Paddle-Signature"))
            .and_then(|v| v.to_str().ok())
        {
            let timestamp = sig_header
                .split(';')
                .find(|p| p.trim().starts_with("ts="))
                .and_then(|p| p.trim()[3..].parse::<u64>().ok())
                .ok_or_else(|| PaddleError::Verification("missing paddle timestamp".into()))?;
            let h1 = sig_header
                .split(';')
                .find(|p| p.trim().starts_with("h1="))
                .map(|p| p.trim()[3..].to_string())
                .ok_or_else(|| PaddleError::Verification("missing paddle h1".into()))?;
            use hmac::{Hmac, Mac};
            use sha2::Sha256;
            let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
                .map_err(|e| PaddleError::Verification(format!("hmac: {e}")))?;
            mac.update(timestamp.to_string().as_bytes());
            mac.update(b".");
            mac.update(body);
            let expected = hex::encode(mac.finalize().into_bytes());
            if !bot_core::session::token::constant_time_eq(expected.as_bytes(), h1.as_bytes()) {
                return Err(PaddleError::Verification(
                    "paddle signature mismatch".into(),
                ));
            }
            let now = chrono::Utc::now().timestamp() as u64;
            if timestamp.abs_diff(now) > 300 {
                return Err(PaddleError::Verification(
                    "paddle timestamp outside window".into(),
                ));
            }
            return serde_json::from_slice(body)
                .map_err(|e| PaddleError::Verification(format!("body is not json: {e}")));
        }
        // generic fallback
        let ts = headers
            .get("x-provider-timestamp")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .ok_or_else(|| PaddleError::Verification("missing x-provider-timestamp".into()))?;
        let sig = headers
            .get("x-provider-signature")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| PaddleError::Verification("missing x-provider-signature".into()))?;
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
            .map_err(|e| PaddleError::Verification(format!("hmac: {e}")))?;
        mac.update(ts.to_string().as_bytes());
        mac.update(b".");
        mac.update(body);
        let expected = hex::encode(mac.finalize().into_bytes());
        if !bot_core::session::token::constant_time_eq(expected.as_bytes(), sig.as_bytes()) {
            return Err(PaddleError::Verification("signature mismatch".into()));
        }
        let now = chrono::Utc::now().timestamp() as u64;
        if ts.abs_diff(now) > 300 {
            return Err(PaddleError::Verification("timestamp outside window".into()));
        }
        serde_json::from_slice(body)
            .map_err(|e| PaddleError::Verification(format!("body is not json: {e}")))
    }
}

impl std::fmt::Debug for PaddleAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaddleAdapter")
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
    fn paddle_not_configured_returns_typed_error() {
        let prev = std::env::var("PADDLE_API_KEY").ok();
        std::env::remove_var("PADDLE_API_KEY");
        let err = PaddleAdapter::from_env();
        assert!(matches!(err, Err(PaddleError::NotConfigured(_))));
        if let Err(PaddleError::NotConfigured(msg)) = err {
            assert!(msg.contains("PADDLE_API_KEY"), "{msg}");
            assert!(msg.contains("no silent fallback"), "{msg}");
        }
        if let Some(v) = prev {
            std::env::set_var("PADDLE_API_KEY", v);
        }
    }

    #[tokio::test]
    async fn paddle_checkout_requires_live_flag() {
        let prev_key = std::env::var("PADDLE_API_KEY").ok();
        let prev_live = std::env::var("LIVE_BILLING").ok();
        std::env::set_var("PADDLE_API_KEY", "paddle_test_12345678");
        std::env::remove_var("LIVE_BILLING");
        let adapter = PaddleAdapter::from_env().expect("configured");
        let intent = PaddleCheckoutIntent {
            organization: OrganizationId::new(),
            plan: PlanCode::Starter,
            idempotency_key: format!("test-{}", uuid::Uuid::new_v4()),
            success_url: None,
            cancel_url: None,
        };
        let res = adapter.create_checkout(&intent).await;
        assert!(matches!(res, Err(PaddleError::Transport(_))));
        if let Some(v) = prev_key {
            std::env::set_var("PADDLE_API_KEY", v);
        } else {
            std::env::remove_var("PADDLE_API_KEY");
        }
        if let Some(v) = prev_live {
            std::env::set_var("LIVE_BILLING", v);
        } else {
            std::env::remove_var("LIVE_BILLING");
        }
    }

    #[test]
    #[ignore]
    fn paddle_live_checkout_ignored_without_live_billing() {
        let live = std::env::var("LIVE_BILLING").unwrap_or_default() == "1"
            && std::env::var("PADDLE_API_KEY")
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
        assert!(
            live,
            "LIVE_BILLING=1 and PADDLE_API_KEY required for live test"
        );
    }

    #[test]
    fn paddle_debug_does_not_leak_key() {
        let adapter =
            PaddleAdapter::with_base_for_test("paddle_live_secret_12345678", PADDLE_API_BASE);
        let dbg = format!("{:?}", adapter);
        assert!(!dbg.contains("paddle_live_secret"));
    }

    async fn mock_paddle_server(status: u16, body: serde_json::Value) -> String {
        use axum::{routing::post, Json, Router};
        let app = Router::new().route(
            PADDLE_CHECKOUT_PATH,
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
    async fn paddle_handles_401_without_leaking_key() {
        let prev_live = std::env::var("LIVE_BILLING").ok();
        std::env::set_var("LIVE_BILLING", "1");
        let base = mock_paddle_server(401, serde_json::json!({"error": "unauthorized"})).await;
        let adapter = PaddleAdapter::with_base_for_test("paddle_live_secret_12345678", base);
        let intent = PaddleCheckoutIntent {
            organization: OrganizationId::new(),
            plan: PlanCode::Starter,
            idempotency_key: format!("idem-{}", uuid::Uuid::new_v4()),
            success_url: None,
            cancel_url: None,
        };
        let res = adapter.create_checkout(&intent).await;
        assert!(matches!(res, Err(PaddleError::Transport(_))));
        if let Err(PaddleError::Transport(msg)) = res {
            assert!(msg.contains("401"));
            assert!(!msg.contains("paddle_live_secret"));
        }
        if let Some(v) = prev_live {
            std::env::set_var("LIVE_BILLING", v);
        } else {
            std::env::remove_var("LIVE_BILLING");
        }
    }

    #[tokio::test]
    async fn paddle_handles_500_without_leaking_key() {
        let prev_live = std::env::var("LIVE_BILLING").ok();
        std::env::set_var("LIVE_BILLING", "1");
        let base = mock_paddle_server(500, serde_json::json!({"error": "internal"})).await;
        let adapter = PaddleAdapter::with_base_for_test("paddle_live_secret_12345678", base);
        let intent = PaddleCheckoutIntent {
            organization: OrganizationId::new(),
            plan: PlanCode::Starter,
            idempotency_key: format!("idem-{}", uuid::Uuid::new_v4()),
            success_url: None,
            cancel_url: None,
        };
        let res = adapter.create_checkout(&intent).await;
        assert!(matches!(res, Err(PaddleError::Transport(_))));
        if let Err(PaddleError::Transport(msg)) = res {
            assert!(msg.contains("500"));
            assert!(!msg.contains("paddle_live_secret"));
        }
        if let Some(v) = prev_live {
            std::env::set_var("LIVE_BILLING", v);
        } else {
            std::env::remove_var("LIVE_BILLING");
        }
    }

    #[tokio::test]
    async fn paddle_handles_malformed_json() {
        let prev_live = std::env::var("LIVE_BILLING").ok();
        std::env::set_var("LIVE_BILLING", "1");
        use axum::{routing::post, Router};
        let app = Router::new().route(
            PADDLE_CHECKOUT_PATH,
            post(|| async { (axum::http::StatusCode::OK, "not json") }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let base = format!("http://{}", addr);
        let adapter = PaddleAdapter::with_base_for_test("paddle_test_12345678", base);
        let intent = PaddleCheckoutIntent {
            organization: OrganizationId::new(),
            plan: PlanCode::Starter,
            idempotency_key: format!("idem-{}", uuid::Uuid::new_v4()),
            success_url: None,
            cancel_url: None,
        };
        let res = adapter.create_checkout(&intent).await;
        assert!(matches!(res, Err(PaddleError::Transport(_))));
        if let Some(v) = prev_live {
            std::env::set_var("LIVE_BILLING", v);
        } else {
            std::env::remove_var("LIVE_BILLING");
        }
    }

    #[tokio::test]
    async fn paddle_handles_missing_url() {
        let prev_live = std::env::var("LIVE_BILLING").ok();
        std::env::set_var("LIVE_BILLING", "1");
        let base = mock_paddle_server(200, serde_json::json!({"data": {"id": "paddle_123"}})).await;
        let adapter = PaddleAdapter::with_base_for_test("paddle_test_12345678", base);
        let intent = PaddleCheckoutIntent {
            organization: OrganizationId::new(),
            plan: PlanCode::Starter,
            idempotency_key: format!("idem-{}", uuid::Uuid::new_v4()),
            success_url: None,
            cancel_url: None,
        };
        let res = adapter.create_checkout(&intent).await;
        assert!(matches!(res, Err(PaddleError::Transport(_))));
        if let Err(PaddleError::Transport(msg)) = res {
            assert!(msg.contains("no url"));
        }
        if let Some(v) = prev_live {
            std::env::set_var("LIVE_BILLING", v);
        } else {
            std::env::remove_var("LIVE_BILLING");
        }
    }

    #[tokio::test]
    async fn paddle_propagates_idempotency_key() {
        let prev_live = std::env::var("LIVE_BILLING").ok();
        std::env::set_var("LIVE_BILLING", "1");
        use axum::{http::HeaderMap, routing::post, Json, Router};
        use std::sync::{Arc, Mutex};
        let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let captured_clone = captured.clone();
        let app = Router::new().route(
            PADDLE_CHECKOUT_PATH,
            post(move |headers: HeaderMap, _body: String| {
                let captured = captured_clone.clone();
                async move {
                    if let Some(v) = headers.get("Idempotency-Key").and_then(|h| h.to_str().ok()) {
                        *captured.lock().unwrap() = Some(v.to_string());
                    }
                    let body = serde_json::json!({"data": {"id": "paddle_123", "url": "https://paddle.com/checkout/paddle_123"}});
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
        let adapter = PaddleAdapter::with_base_for_test("paddle_test_12345678", base);
        let key = format!("idem-{}", uuid::Uuid::new_v4());
        let intent = PaddleCheckoutIntent {
            organization: OrganizationId::new(),
            plan: PlanCode::Starter,
            idempotency_key: key.clone(),
            success_url: None,
            cancel_url: None,
        };
        let res = adapter
            .create_checkout(&intent)
            .await
            .expect("should succeed");
        assert_eq!(res.session_id, "paddle_123");
        assert_eq!(*captured.lock().unwrap(), Some(key));
        if let Some(v) = prev_live {
            std::env::set_var("LIVE_BILLING", v);
        } else {
            std::env::remove_var("LIVE_BILLING");
        }
    }

    #[tokio::test]
    async fn paddle_missing_session_id_is_an_error() {
        let prev_live = std::env::var("LIVE_BILLING").ok();
        std::env::set_var("LIVE_BILLING", "1");
        // 200 with a URL but no id: the fabricated "paddle_session" placeholder
        // is gone — an unusable provider identity must fail closed.
        let base = mock_paddle_server(
            200,
            serde_json::json!({"data": {"url": "https://paddle.com/checkout/x"}}),
        )
        .await;
        let adapter = PaddleAdapter::with_base_for_test("paddle_test_12345678", base);
        let intent = PaddleCheckoutIntent {
            organization: OrganizationId::new(),
            plan: PlanCode::Starter,
            idempotency_key: format!("idem-{}", uuid::Uuid::new_v4()),
            success_url: None,
            cancel_url: None,
        };
        let res = adapter.create_checkout(&intent).await;
        assert!(matches!(res, Err(PaddleError::Transport(_))));
        if let Err(PaddleError::Transport(msg)) = res {
            assert!(msg.contains("no session id"), "{msg}");
        }
        if let Some(v) = prev_live {
            std::env::set_var("LIVE_BILLING", v);
        } else {
            std::env::remove_var("LIVE_BILLING");
        }
    }

    #[tokio::test]
    async fn paddle_timeout_is_a_bounded_classified_error() {
        let prev_live = std::env::var("LIVE_BILLING").ok();
        std::env::set_var("LIVE_BILLING", "1");
        use axum::{routing::post, Router};
        let app = Router::new().route(
            PADDLE_CHECKOUT_PATH,
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
            PaddleAdapter::with_base_and_timeout_for_test("paddle_test_12345678", base, 1).unwrap();
        let started = std::time::Instant::now();
        let intent = PaddleCheckoutIntent {
            organization: OrganizationId::new(),
            plan: PlanCode::Starter,
            idempotency_key: format!("idem-{}", uuid::Uuid::new_v4()),
            success_url: None,
            cancel_url: None,
        };
        let res = adapter.create_checkout(&intent).await;
        assert!(
            matches!(res, Err(PaddleError::Transport(_))),
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
