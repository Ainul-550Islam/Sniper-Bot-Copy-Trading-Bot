//! Transactional email subsystem (GAP-MAP v2 P1).
//!
//! Before this module existed there was NO email code anywhere in the
//! server, so invites, password resets, verification and alerts could not
//! be delivered. The design:
//!
//! * Handlers NEVER send email directly — they enqueue a row in
//!   `email_outbox` (migration 0047). [`outbox::OutboxDispatcher`] owns
//!   delivery: retry with exponential backoff, dedupe via `dedup_key`,
//!   dead-letter after `max_attempts`, rate limiting.
//! * Delivery is abstracted behind [`EmailProvider`]:
//!   * [`http_provider::HttpEmailProvider`] — Resend / Postmark / SES v2
//!     HTTP APIs (TLS by construction);
//!   * [`smtp::SmtpProvider`] — SMTP submission with STARTTLS or implicit
//!     TLS. PLAINTEXT IS REFUSED: the provider errors instead of sending.
//! * Secrets (API keys, SMTP passwords) live in env vars named by config;
//!   they are never logged and never stored in the outbox.

pub mod http_provider;
pub mod outbox;
pub mod smtp;
pub mod templates;

use async_trait::async_trait;

use templates::Rendered;

/// One logical email to deliver.
#[derive(Debug, Clone)]
pub struct EmailMessage {
    /// Envelope recipient (RFC 5321).
    pub to: String,
    /// Envelope sender ("From" address).
    pub from: String,
    /// Rendered content.
    pub rendered: Rendered,
    /// Idempotency key for the outbox (one row per logical event).
    pub dedup_key: String,
}

/// Provider-level delivery failure. Storage failures are `sqlx::Error` at
/// the outbox layer and are NOT this type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmailError {
    /// Provider configuration is missing/invalid (bad env var, empty key).
    Config(String),
    /// The provider rejected the message (4xx/5xx, API error body). The
    /// string is a sanitised provider message — never the credential.
    Rejected(String),
    /// Network/transport failure — retryable.
    Transport(String),
    /// TLS was required but not available/configured.
    TlsRequired(String),
    /// Request timed out — retryable.
    Timeout,
}

impl std::fmt::Display for EmailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmailError::Config(m) => write!(f, "email provider configuration error: {m}"),
            EmailError::Rejected(m) => write!(f, "email rejected by provider: {m}"),
            EmailError::Transport(m) => write!(f, "email transport failure: {m}"),
            EmailError::TlsRequired(m) => write!(f, "email requires TLS: {m}"),
            EmailError::Timeout => write!(f, "email delivery timed out"),
        }
    }
}

impl std::error::Error for EmailError {}

impl EmailError {
    /// Retryable failures go back to the outbox with backoff; permanent
    /// ones dead-letter immediately.
    pub fn is_retryable(&self) -> bool {
        matches!(self, EmailError::Transport(_) | EmailError::Timeout)
    }
}

/// An email delivery backend.
#[async_trait]
pub trait EmailProvider: Send + Sync {
    /// Deliver one message. Returns the provider's message id when one is
    /// available (journaled on the outbox row).
    async fn send(&self, message: &EmailMessage) -> Result<Option<String>, EmailError>;

    /// Stable provider label for logs/metrics (never contains secrets).
    fn name(&self) -> &'static str;
}

/// Deployment configuration for the email subsystem. Every secret is the
/// NAME of an environment variable, never a literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailConfig {
    /// Master switch. When off, the outbox dispatcher is not started and
    /// handlers report email as unavailable (rows are still enqueue-able,
    /// so a later enable delivers them).
    pub enabled: bool,
    /// Sender address stamped on outgoing mail.
    pub from_address: String,
    /// Which backend to use.
    pub backend: EmailBackend,
    /// Per-minute send ceiling (0 = unset → the dispatcher uses its
    /// conservative default). Protects provider quotas AND prevents a
    /// buggy loop from blasting the list.
    pub per_minute_limit: u32,
}

/// Backend selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmailBackend {
    /// Resend HTTP API. Env var holding the API key.
    Resend { api_key_env: String },
    /// Postmark HTTP API. Env var holding the server token.
    Postmark { token_env: String },
    /// AWS SES v2 HTTP API with SigV4 (IAM long-term credentials).
    Ses {
        region: String,
        access_key_env: String,
        secret_key_env: String,
    },
    /// SMTP submission over TLS (STARTTLS or implicit). Plaintext refused.
    Smtp {
        host: String,
        port: u16,
        /// `true` = TLS from the first byte (port 465 style);
        /// `false` = STARTTLS upgrade (port 587 style). Both are TLS.
        implicit_tls: bool,
        username: String,
        password_env: String,
        /// PEM bundle for trusted roots. `None` → try common OS paths.
        ca_bundle_path: Option<String>,
    },
    /// Deliver to a local directory as `.eml` files (dev/test only).
    /// Refused unless `allow_file_backend` is set, so production cannot
    /// silently mis-configure into it.
    File {
        directory: String,
        allow_file_backend: bool,
    },
}

impl Default for EmailConfig {
    fn default() -> Self {
        EmailConfig {
            enabled: false,
            from_address: "no-reply@localhost".to_string(),
            backend: EmailBackend::Resend {
                api_key_env: "RESEND_API_KEY".to_string(),
            },
            per_minute_limit: 0,
        }
    }
}

impl EmailConfig {
    /// Opt-in deployment configuration. Provider secrets remain in the named
    /// environment variables; they are never copied into logs or config data.
    pub fn from_env() -> Result<Self, EmailError> {
        let enabled = match std::env::var("EMAIL_ENABLED") {
            Ok(value) if value.eq_ignore_ascii_case("true") => true,
            Ok(value) if value.eq_ignore_ascii_case("false") => false,
            Ok(_) => {
                return Err(EmailError::Config(
                    "EMAIL_ENABLED must be true or false".into(),
                ))
            }
            Err(std::env::VarError::NotPresent) => false,
            Err(error) => {
                return Err(EmailError::Config(format!(
                    "EMAIL_ENABLED is not valid Unicode: {error}"
                )))
            }
        };
        let mut config = Self {
            enabled,
            ..Self::default()
        };
        if !enabled {
            return Ok(config);
        }
        config.from_address = std::env::var("EMAIL_FROM")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                EmailError::Config("EMAIL_FROM must be set when EMAIL_ENABLED=true".into())
            })?;
        let backend = std::env::var("EMAIL_BACKEND").unwrap_or_else(|_| "resend".into());
        config.backend = match backend.trim().to_ascii_lowercase().as_str() {
            "resend" => EmailBackend::Resend {
                api_key_env: std::env::var("EMAIL_RESEND_API_KEY_ENV")
                    .unwrap_or_else(|_| "RESEND_API_KEY".into()),
            },
            "postmark" => EmailBackend::Postmark {
                token_env: std::env::var("EMAIL_POSTMARK_TOKEN_ENV")
                    .unwrap_or_else(|_| "POSTMARK_SERVER_TOKEN".into()),
            },
            "ses" => EmailBackend::Ses {
                region: std::env::var("EMAIL_SES_REGION").unwrap_or_default(),
                access_key_env: std::env::var("EMAIL_SES_ACCESS_KEY_ENV")
                    .unwrap_or_else(|_| "AWS_ACCESS_KEY_ID".into()),
                secret_key_env: std::env::var("EMAIL_SES_SECRET_KEY_ENV")
                    .unwrap_or_else(|_| "AWS_SECRET_ACCESS_KEY".into()),
            },
            "smtp" => {
                let host = std::env::var("EMAIL_SMTP_HOST").unwrap_or_default();
                let port = std::env::var("EMAIL_SMTP_PORT")
                    .unwrap_or_else(|_| "587".into())
                    .parse::<u16>()
                    .map_err(|_| {
                        EmailError::Config("EMAIL_SMTP_PORT must be a valid TCP port".into())
                    })?;
                let implicit_tls = match std::env::var("EMAIL_SMTP_IMPLICIT_TLS").as_deref() {
                    Ok("true") => true,
                    Ok("false") | Err(_) => false,
                    _ => {
                        return Err(EmailError::Config(
                            "EMAIL_SMTP_IMPLICIT_TLS must be true or false".into(),
                        ))
                    }
                };
                let username = std::env::var("EMAIL_SMTP_USERNAME").unwrap_or_default();
                let password_env = std::env::var("EMAIL_SMTP_PASSWORD_ENV")
                    .unwrap_or_else(|_| "SMTP_PASSWORD".into());
                let ca_bundle_path = std::env::var("EMAIL_SMTP_CA_BUNDLE").ok();
                EmailBackend::Smtp {
                    host,
                    port,
                    implicit_tls,
                    username,
                    password_env,
                    ca_bundle_path,
                }
            }
            _ => {
                return Err(EmailError::Config(
                    "EMAIL_BACKEND must be resend, postmark, ses, or smtp".into(),
                ))
            }
        };
        config.per_minute_limit = std::env::var("EMAIL_PER_MINUTE_LIMIT")
            .unwrap_or_else(|_| "0".into())
            .parse::<u32>()
            .map_err(|_| {
                EmailError::Config("EMAIL_PER_MINUTE_LIMIT must be a non-negative integer".into())
            })?;
        Ok(config)
    }

    pub fn build_provider(&self) -> Result<std::sync::Arc<dyn EmailProvider>, EmailError> {
        use std::sync::Arc;
        let provider: Arc<dyn EmailProvider> = match &self.backend {
            EmailBackend::Resend { api_key_env } => {
                Arc::new(http_provider::HttpEmailProvider::resend(api_key_env)?)
            }
            EmailBackend::Postmark { token_env } => {
                Arc::new(http_provider::HttpEmailProvider::postmark(token_env)?)
            }
            EmailBackend::Ses {
                region,
                access_key_env,
                secret_key_env,
            } => Arc::new(http_provider::HttpEmailProvider::ses(
                region,
                access_key_env,
                secret_key_env,
            )?),
            EmailBackend::Smtp {
                host,
                port,
                implicit_tls,
                username,
                password_env,
                ca_bundle_path,
            } => Arc::new(smtp::SmtpProvider::new(
                host,
                *port,
                *implicit_tls,
                username,
                password_env,
                ca_bundle_path.as_deref(),
            )?),
            EmailBackend::File { .. } => {
                return Err(EmailError::Config(
                    "file email backend is not wired for production startup".into(),
                ))
            }
        };
        Ok(provider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryability_matches_backoff_policy() {
        assert!(EmailError::Transport("conn reset".into()).is_retryable());
        assert!(EmailError::Timeout.is_retryable());
        assert!(!EmailError::Rejected("550 no such user".into()).is_retryable());
        assert!(!EmailError::Config("missing key".into()).is_retryable());
        assert!(!EmailError::TlsRequired("plaintext".into()).is_retryable());
    }

    #[test]
    fn default_config_is_disabled() {
        assert!(!EmailConfig::default().enabled, "email must be opt-in");
    }
}
