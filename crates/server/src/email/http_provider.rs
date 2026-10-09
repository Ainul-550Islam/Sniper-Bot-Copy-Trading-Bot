//! HTTP email providers: Resend, Postmark, AWS SES v2 (GAP-MAP v2 P1).
//!
//! All three are HTTPS-only by construction (reqwest with rustls). Secrets
//! are read from the env var NAMES given in configuration at build time and
//! never appear in logs, errors, or metric labels.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;

use super::{EmailError, EmailMessage, EmailProvider};

/// Request ceiling for one delivery attempt (the outbox retries).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// The three HTTP backends.
#[derive(Debug, Clone)]
pub enum HttpEmailProvider {
    Resend {
        client: Client,
        api_key: String,
        endpoint: String,
    },
    Postmark {
        client: Client,
        server_token: String,
        endpoint: String,
    },
    Ses {
        client: Client,
        region: String,
        access_key_id: String,
        secret_access_key: String,
        endpoint: String,
    },
}

impl HttpEmailProvider {
    /// Resend provider from the configured env var name.
    pub fn resend(api_key_env: &str) -> Result<Self, EmailError> {
        let api_key = read_env(api_key_env)?;
        Ok(HttpEmailProvider::Resend {
            client: build_client()?,
            api_key,
            endpoint: "https://api.resend.com/emails".to_string(),
        })
    }

    /// Postmark provider from the configured env var name.
    pub fn postmark(token_env: &str) -> Result<Self, EmailError> {
        let server_token = read_env(token_env)?;
        Ok(HttpEmailProvider::Postmark {
            client: build_client()?,
            server_token,
            endpoint: "https://api.postmarkapp.com/email".to_string(),
        })
    }

    /// SES v2 provider from the configured env var names.
    pub fn ses(
        region: &str,
        access_key_env: &str,
        secret_key_env: &str,
    ) -> Result<Self, EmailError> {
        let region = region.trim().to_ascii_lowercase();
        if region.is_empty() {
            return Err(EmailError::Config("SES region must not be empty".into()));
        }
        let access_key_id = read_env(access_key_env)?;
        let secret_access_key = read_env(secret_key_env)?;
        let endpoint = format!("https://email.{region}.amazonaws.com/v2/email/outbound-emails");
        Ok(HttpEmailProvider::Ses {
            client: build_client()?,
            region,
            access_key_id,
            secret_access_key,
            endpoint,
        })
    }
}

fn build_client() -> Result<Client, EmailError> {
    Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| EmailError::Config(format!("http client build failed: {e}")))
}

fn read_env(name: &str) -> Result<String, EmailError> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| EmailError::Config(format!("env var {name} is not set or empty")))
}

#[async_trait]
impl EmailProvider for HttpEmailProvider {
    fn name(&self) -> &'static str {
        match self {
            HttpEmailProvider::Resend { .. } => "resend",
            HttpEmailProvider::Postmark { .. } => "postmark",
            HttpEmailProvider::Ses { .. } => "ses",
        }
    }

    async fn send(&self, message: &EmailMessage) -> Result<Option<String>, EmailError> {
        match self {
            HttpEmailProvider::Resend {
                client,
                api_key,
                endpoint,
            } => {
                let body = serde_json::json!({
                    "from": message.from,
                    "to": [message.to],
                    "subject": message.rendered.subject,
                    "text": message.rendered.body_text,
                    "html": message.rendered.body_html,
                });
                let resp = client
                    .post(endpoint)
                    .bearer_auth(api_key)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| transport(e))?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if status.is_success() {
                    let id = serde_json::from_str::<serde_json::Value>(&text)
                        .ok()
                        .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(str::to_string));
                    return Ok(id);
                }
                Err(rejection(status.as_u16(), &text))
            }
            HttpEmailProvider::Postmark {
                client,
                server_token,
                endpoint,
            } => {
                let body = serde_json::json!({
                    "From": message.from,
                    "To": message.to,
                    "Subject": message.rendered.subject,
                    "TextBody": message.rendered.body_text,
                    "HtmlBody": message.rendered.body_html,
                });
                let resp = client
                    .post(endpoint)
                    .header("X-Postmark-Server-Token", server_token)
                    .header("Accept", "application/json")
                    .json(&body)
                    .send()
                    .await
                    .map_err(transport)?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if status.is_success() {
                    let id = serde_json::from_str::<serde_json::Value>(&text)
                        .ok()
                        .and_then(|v| v.get("MessageID").map(|m| m.to_string()));
                    return Ok(id);
                }
                Err(rejection(status.as_u16(), &text))
            }
            HttpEmailProvider::Ses {
                client,
                region,
                access_key_id,
                secret_access_key,
                endpoint,
            } => {
                let body = serde_json::json!({
                    "FromEmailAddress": message.from,
                    "Destination": { "ToAddresses": [message.to] },
                    "Content": {
                        "Simple": {
                            "Subject": { "Data": message.rendered.subject, "Charset": "UTF-8" },
                            "Body": {
                                "Text": { "Data": message.rendered.body_text, "Charset": "UTF-8" },
                                "Html": { "Data": message.rendered.body_html, "Charset": "UTF-8" },
                            }
                        }
                    }
                });
                let payload = serde_json::to_vec(&body)
                    .map_err(|e| EmailError::Config(format!("ses body encode: {e}")))?;
                let host = endpoint
                    .trim_start_matches("https://")
                    .split('/')
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let (headers, authorization) =
                    sign_sigv4(region, access_key_id, secret_access_key, &host, &payload);
                let mut request = client
                    .post(endpoint)
                    .header("Content-Type", "application/json")
                    .header("Authorization", authorization);
                for (key, value) in headers {
                    request = request.header(key, value);
                }
                let resp = request.body(payload).send().await.map_err(transport)?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if status.is_success() {
                    let id = serde_json::from_str::<serde_json::Value>(&text)
                        .ok()
                        .and_then(|v| v.get("MessageId").and_then(|i| i.as_str()).map(str::to_string));
                    return Ok(id);
                }
                Err(rejection(status.as_u16(), &text))
            }
        }
    }
}

/// Map a reqwest error to retryable transport / timeout.
fn transport(e: reqwest::Error) -> EmailError {
    if e.is_timeout() {
        EmailError::Timeout
    } else {
        EmailError::Transport(e.to_string())
    }
}

/// Build a provider rejection from an HTTP status + sanitised body. The
/// first 300 chars are kept so operators can act on it; bodies may contain
/// the recipient address (fine — it is the operator's own data) but never
/// our credential (providers do not echo auth headers in error bodies).
fn rejection(status: u16, body: &str) -> EmailError {
    let clean: String = body.chars().take(300).collect();
    // 429 = provider rate limit: retryable after backoff. 5xx likewise.
    if status == 429 || (500..600).contains(&status) {
        return EmailError::Transport(format!("provider http {status}: {clean}"));
    }
    EmailError::Rejected(format!("provider http {status}: {clean}"))
}

// ---------------------------------------------------------------------------
// AWS SigV4 (minimal, for the SES v2 JSON endpoint)
// ---------------------------------------------------------------------------

/// Sign one POST request. Returns the extra date headers and the
/// Authorization header value.
fn sign_sigv4(
    region: &str,
    access_key_id: &str,
    secret_access_key: &str,
    host: &str,
    payload: &[u8],
) -> (Vec<(&'static str, String)>, String) {
    use hmac::{Hmac, Mac};
    use sha2::{Digest, Sha256};
    type HmacSha256 = Hmac<Sha256>;

    let now = chrono::Utc::now();
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let datestamp = now.format("%Y%m%d").to_string();

    let payload_hash = hex_digest(&Sha256::digest(payload));

    let canonical_headers = format!(
        "content-type:application/json\nhost:{host}\nx-amz-date:{amz_date}\n"
    );
    let signed_headers = "content-type;host;x-amz-date";
    let canonical_request = format!(
        "POST\n/v2/email/outbound-emails\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );
    let scope = format!("{datestamp}/{region}/ses/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex_digest(&Sha256::digest(canonical_request.as_bytes()))
    );

    let k_date = hmac_vec(format!("AWS4{secret_access_key}").as_bytes(), datestamp.as_bytes());
    let k_region = hmac_vec(&k_date, region.as_bytes());
    let k_service = hmac_vec(&k_region, b"ses");
    let k_signing = hmac_vec(&k_service, b"aws4_request");
    let signature = hex_digest(&hmac_vec(&k_signing, string_to_sign.as_bytes()));

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/{scope}, SignedHeaders={signed_headers}, Signature={signature}"
    );

    fn hmac_vec(key: &[u8], data: &[u8]) -> Vec<u8> {
        let mut mac = <HmacSha256 as Mac>::new_from_slice(key)
            .expect("HMAC accepts keys of any length");
        mac.update(data);
        mac.finalize().into_bytes().to_vec()
    }

    (
        vec![("X-Amz-Date", amz_date)],
        authorization,
    )
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_refuse_missing_credentials_without_panicking() {
        std::env::remove_var("TEST_EMAIL_KEY_ABSENT_XYZ");
        let e = HttpEmailProvider::resend("TEST_EMAIL_KEY_ABSENT_XYZ").unwrap_err();
        assert!(matches!(e, EmailError::Config(_)));
        let e = HttpEmailProvider::postmark("TEST_EMAIL_KEY_ABSENT_XYZ").unwrap_err();
        assert!(matches!(e, EmailError::Config(_)));
        let e = HttpEmailProvider::ses("us-east-1", "TEST_EMAIL_KEY_ABSENT_XYZ", "TEST_EMAIL_KEY_ABSENT_XYZ")
            .unwrap_err();
        assert!(matches!(e, EmailError::Config(_)));
        let e = HttpEmailProvider::ses("", "A", "B").unwrap_err();
        assert!(matches!(e, EmailError::Config(_)));
    }

    #[test]
    fn rejection_classifies_retryable_vs_permanent() {
        assert!(rejection(429, "slow down").is_retryable());
        assert!(rejection(503, "try later").is_retryable());
        assert!(!rejection(400, "bad body").is_retryable());
        assert!(!rejection(406, "not acceptable").is_retryable());
        // Body is truncated, never unbounded.
        let long = "x".repeat(10_000);
        if let EmailError::Rejected(msg) = rejection(400, &long) {
            assert!(msg.len() < 400);
        } else {
            panic!("expected Rejected");
        }
    }

    #[test]
    fn sigv4_produces_stable_structure() {
        let (headers, auth) = sign_sigv4(
            "eu-west-1",
            "AKIAEXAMPLE",
            "secret",
            "email.eu-west-1.amazonaws.com",
            b"{}",
        );
        assert!(headers.iter().any(|(k, _)| *k == "X-Amz-Date"));
        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIAEXAMPLE/"));
        assert!(auth.contains("/eu-west-1/ses/aws4_request"));
        assert!(auth.contains("SignedHeaders=content-type;host;x-amz-date"));
        // Signature is 64 hex chars.
        let sig = auth.rsplit("Signature=").next().unwrap_or("");
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn sending_with_bad_credentials_surfaces_a_clean_error() {
        // An unreachable host must surface as Transport/Timeout, never panic.
        std::env::set_var("TEST_EMAIL_RESEND_KEY", "rk_test_invalid");
        let provider = HttpEmailProvider::resend("TEST_EMAIL_RESEND_KEY").unwrap();
        let message = EmailMessage {
            to: "nobody@example.com".into(),
            from: "no-reply@example.com".into(),
            rendered: crate::email::templates::email_verification(
                "nobody@example.com",
                "https://example.com/verify",
                30,
            ),
            dedup_key: "dedup-1".into(),
        };
        // Real network call against the public API with an invalid key:
        // expect a clean Rejected (401/403/422) — but if the sandbox has
        // no network, Transport/Timeout is equally acceptable. Panic is
        // the only failure of this test.
        let result = provider.send(&message).await;
        assert!(result.is_err(), "invalid key must not succeed");
        std::env::remove_var("TEST_EMAIL_RESEND_KEY");
    }
}
