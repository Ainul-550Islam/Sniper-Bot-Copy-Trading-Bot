//! Typed asynchronous SaaS client (BATCH file 26).
//!
//! Implements methods for authentication/session, organization selection, tenant status,
//! billing/checkout, invoices, usage, custody public state, wallet binding metadata,
//! audit export, WebSocket endpoint construction WITHOUT placing secrets in URLs.
//! Uses Authorization headers or first-frame authentication. Typed errors and retry classification.
//! No business secrets in logs or Debug output.

use std::fmt;
use std::sync::Arc;

use serde::{de::DeserializeOwned, Serialize};
use url::Url;

use crate::error::{SdkError, SdkErrorKind};
use crate::models::{
    BillingCheckoutRequest, BillingCheckoutResponse, InvoiceView, OrganizationView,
    SessionResponse, TenantStatus, UsageEntry, UserProfile, WalletBindingView,
};

/// SDK client configuration.
#[derive(Clone)]
pub struct SaasClientConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub session_token: Option<String>,
    pub timeout_secs: u64,
}

impl fmt::Debug for SaasClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SaasClientConfig")
            .field("base_url", &self.base_url)
            .field("has_api_key", &self.api_key.is_some())
            .field("has_session_token", &self.session_token.is_some())
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

/// Builder for SaasClient.
pub struct SaasClientBuilder {
    base_url: Option<String>,
    api_key: Option<String>,
    session_token: Option<String>,
    timeout_secs: u64,
}

impl SaasClientBuilder {
    pub fn new() -> Self {
        Self {
            base_url: None,
            api_key: None,
            session_token: None,
            timeout_secs: 30,
        }
    }
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }
    /// Set API key — stored privately, never rendered in Debug or logs.
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }
    pub fn session_token(mut self, token: impl Into<String>) -> Self {
        self.session_token = Some(token.into());
        self
    }
    pub fn timeout_secs(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    pub fn build(self) -> Result<SaasClient, SdkError> {
        let base = self
            .base_url
            .ok_or_else(|| SdkError::new(SdkErrorKind::InvalidRequest, "base_url is required"))?;
        // Validate URL
        Url::parse(&base).map_err(|e| {
            SdkError::new(
                SdkErrorKind::InvalidRequest,
                format!("invalid base_url: {}", e),
            )
        })?;
        let config = SaasClientConfig {
            base_url: base.trim_end_matches('/').to_string(),
            api_key: self.api_key,
            session_token: self.session_token,
            timeout_secs: self.timeout_secs,
        };
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| {
                SdkError::new(
                    SdkErrorKind::Transport,
                    format!("failed to build http client: {}", e),
                )
            })?;
        Ok(SaasClient {
            config: Arc::new(config),
            http: Arc::new(http),
        })
    }
}

impl Default for SaasClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Typed SaaS client.
#[derive(Clone)]
pub struct SaasClient {
    config: Arc<SaasClientConfig>,
    http: Arc<reqwest::Client>,
}

impl fmt::Debug for SaasClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SaasClient")
            .field("base_url", &self.config.base_url)
            .field(
                "has_credentials",
                &(self.config.api_key.is_some() || self.config.session_token.is_some()),
            )
            .finish()
    }
}

impl SaasClient {
    /// Build with builder.
    pub fn builder() -> SaasClientBuilder {
        SaasClientBuilder::new()
    }

    fn auth_header_value(&self) -> Option<String> {
        if let Some(token) = &self.config.session_token {
            return Some(format!("Bearer {}", token));
        }
        if let Some(key) = &self.config.api_key {
            return Some(format!("Bearer {}", key));
        }
        None
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.config.base_url, path)
    }

    pub(crate) async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, SdkError> {
        self.request(reqwest::Method::GET, path, Option::<&()>::None)
            .await
    }

    pub(crate) async fn post<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, SdkError> {
        self.request(reqwest::Method::POST, path, Some(body)).await
    }

    async fn request<T: DeserializeOwned, B: Serialize>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T, SdkError> {
        let url = self.url(path);
        let mut req = self.http.request(method, &url);
        if let Some(auth) = self.auth_header_value() {
            req = req.header("Authorization", auth);
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await.map_err(|e| {
            SdkError::new(SdkErrorKind::Transport, format!("request failed: {}", e))
        })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| {
            SdkError::new(
                SdkErrorKind::Transport,
                format!("failed to read response: {}", e),
            )
        })?;

        if status.is_success() {
            serde_json::from_slice::<T>(&bytes).map_err(|e| {
                SdkError::new(
                    SdkErrorKind::Decode,
                    format!("failed to decode response: {}", e),
                )
            })
        } else {
            // Try to parse error body
            let body_str = String::from_utf8_lossy(&bytes).to_string();
            let kind = match status.as_u16() {
                400 => SdkErrorKind::InvalidRequest,
                401 => SdkErrorKind::Unauthorized,
                403 => SdkErrorKind::Forbidden,
                404 => SdkErrorKind::NotFound,
                429 => SdkErrorKind::RateLimited,
                500..=599 => SdkErrorKind::Server,
                _ => SdkErrorKind::Unknown,
            };
            Err(SdkError::new(
                kind,
                format!(
                    "{} {}: {}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("error"),
                    body_str
                ),
            )
            .with_status(status.as_u16()))
        }
    }

    // --- Authentication / session ------------------------------------------------

    pub async fn login(&self, email: &str, password: &str) -> Result<SessionResponse, SdkError> {
        let body = serde_json::json!({"email": email, "password": password});
        self.request(reqwest::Method::POST, "/api/saas/sessions", Some(&body))
            .await
    }

    pub async fn current_user(&self) -> Result<UserProfile, SdkError> {
        self.get("/api/saas/users/me").await
    }

    pub async fn logout(&self) -> Result<(), SdkError> {
        let _: serde_json::Value = self
            .request(
                reqwest::Method::POST,
                "/api/saas/users/me/logout",
                Some(&serde_json::json!({})),
            )
            .await?;
        Ok(())
    }

    // --- Organization selection / tenant status ---------------------------------

    pub async fn get_organization(&self, id: &str) -> Result<OrganizationView, SdkError> {
        self.get(&format!("/api/saas/organizations/{}", id)).await
    }

    pub async fn tenant_status(&self, id: &str) -> Result<TenantStatus, SdkError> {
        self.get(&format!("/api/saas/organizations/{}/lifecycle", id))
            .await
    }

    pub async fn list_members(&self, org_id: &str) -> Result<Vec<UserProfile>, SdkError> {
        let val: serde_json::Value = self
            .get(&format!("/api/saas/organizations/{}/members", org_id))
            .await?;
        Ok(val
            .get("members")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default())
    }

    // --- Billing / checkout / invoices / usage ---------------------------------

    pub async fn create_checkout(
        &self,
        req: BillingCheckoutRequest,
    ) -> Result<BillingCheckoutResponse, SdkError> {
        self.post("/api/saas/checkout", &req).await
    }

    pub async fn list_invoices(&self) -> Result<Vec<InvoiceView>, SdkError> {
        let val: serde_json::Value = self.get("/api/saas/invoices").await?;
        Ok(val
            .get("invoices")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default())
    }

    pub async fn get_invoice(&self, id: &str) -> Result<InvoiceView, SdkError> {
        self.get(&format!("/api/saas/invoices/{}", id)).await
    }

    pub async fn get_usage(&self, period: Option<&str>) -> Result<Vec<UsageEntry>, SdkError> {
        let path = if let Some(p) = period {
            format!("/api/saas/exports?kind=usage&period={}", p)
        } else {
            "/api/saas/exports?kind=usage".into()
        };
        let val: serde_json::Value = self.get(&path).await?;
        Ok(val
            .get("data")
            .and_then(|v| v.get("usage"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default())
    }

    // --- Wallet binding metadata -------------------------------------------------

    pub async fn wallet_bindings(&self) -> Result<Vec<WalletBindingView>, SdkError> {
        let val: serde_json::Value = self.get("/api/saas/wallets").await?; // existing wallet_access endpoint
                                                                           // Try new custody path as well — fallback to wallet_access
        if val.is_array() {
            return serde_json::from_value(val)
                .map_err(|e| SdkError::new(SdkErrorKind::Decode, format!("{}", e)));
        }
        // Try exports fallback
        let val2: serde_json::Value = self.get("/api/saas/exports?kind=wallets").await?;
        Ok(val2
            .get("data")
            .and_then(|v| v.get("wallets"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default())
    }

    // --- Audit export -------------------------------------------------------------

    pub async fn audit_export(&self) -> Result<serde_json::Value, SdkError> {
        self.get("/api/saas/exports?kind=audit").await
    }

    // --- WebSocket endpoint construction WITHOUT secrets in URLs ------------------

    /// Build WebSocket URL for SaaS events. Never includes secret in query string.
    /// Caller must use Authorization header or first-frame auth.
    pub fn websocket_url(&self) -> String {
        let base = self
            .config
            .base_url
            .replacen("http://", "ws://", 1)
            .replacen("https://", "wss://", 1);
        format!("{}/api/saas/events", base)
    }

    /// Build WebSocket URL with first-frame auth hint? No — we deliberately do NOT embed token.
    /// This method returns the URL and the auth header separately.
    pub fn websocket_auth_header(&self) -> Option<String> {
        self.auth_header_value()
    }

    // --- Helpers ----------------------------------------------------------------

    pub fn base_url(&self) -> &str {
        &self.config.base_url
    }

    /// Whether the client has credentials.
    pub fn has_credentials(&self) -> bool {
        self.config.api_key.is_some() || self.config.session_token.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_requires_base_url() {
        let err = SaasClient::builder().build().unwrap_err();
        assert_eq!(err.kind, crate::error::SdkErrorKind::InvalidRequest);
    }

    #[test]
    fn client_debug_never_emits_secrets() {
        let client = SaasClient::builder()
            .base_url("https://api.example.com")
            .api_key("sk_live_secret_12345")
            .build()
            .unwrap();
        let dbg = format!("{:?}", client);
        assert!(
            !dbg.contains("sk_live_secret"),
            "Debug leaked secret: {}",
            dbg
        );
        assert!(dbg.contains("https://api.example.com"));
    }

    #[test]
    fn websocket_url_never_contains_secret() {
        let client = SaasClient::builder()
            .base_url("https://api.example.com")
            .api_key("sk_secret")
            .session_token("sess_token")
            .build()
            .unwrap();
        let ws = client.websocket_url();
        assert!(!ws.contains("sk_secret"));
        assert!(!ws.contains("sess_token"));
        assert!(!ws.contains("token="));
        assert!(!ws.contains("key="));
        assert!(ws.contains("wss://"));
        assert!(ws.ends_with("/api/saas/events"));
        // Auth is via header, not query string
        let header = client.websocket_auth_header().unwrap();
        assert!(header.starts_with("Bearer "));
        assert!(
            !ws.contains('?'),
            "websocket URL must not have query string with secret, got {}",
            ws
        );
    }

    #[test]
    fn base_url_trailing_slash_normalized() {
        let client = SaasClient::builder()
            .base_url("https://api.example.com/")
            .build()
            .unwrap();
        assert_eq!(client.base_url(), "https://api.example.com");
    }

    #[test]
    fn auth_header_prefers_session_token() {
        let client = SaasClient::builder()
            .base_url("https://api.example.com")
            .api_key("api_key_value")
            .session_token("session_value")
            .build()
            .unwrap();
        let header = client.websocket_auth_header().unwrap();
        assert!(header.contains("session_value"), "session token should win");
        assert!(!header.contains("api_key_value"));
    }
}
