//! SMTP submission provider — TLS MANDATORY (GAP-MAP v2 P1).
//!
//! Two connection shapes, both encrypted:
//! * `implicit_tls = true`  — TLS from the first byte (port 465 style);
//! * `implicit_tls = false` — plaintext greeting + STARTTLS upgrade
//!   (port 587 style); if the server does not OFFER STARTTLS the send
//!   FAILS with [`EmailError::TlsRequired`] — this provider never falls
//!   back to plaintext, and neither EHLO capability nor configuration can
//!   talk it into it.
//!
//! Trusted roots come from a PEM bundle (configured path, or the common
//! OS locations). Secrets (the SMTP password) are read from the env var
//! named by configuration and never logged.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufStream};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;

use super::{EmailError, EmailMessage, EmailProvider};

/// One phase of the protocol may take at most this long.
const PHASE_TIMEOUT: Duration = Duration::from_secs(15);

/// A TLS-only SMTP submission client.
#[derive(Clone)]
pub struct SmtpProvider {
    host: String,
    port: u16,
    implicit_tls: bool,
    username: String,
    password: String,
    connector: TlsConnector,
}

// Manual Debug: `TlsConnector` has no Debug impl, and the password must never
// reach a log line through a derived formatter.
impl std::fmt::Debug for SmtpProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpProvider")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("implicit_tls", &self.implicit_tls)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl SmtpProvider {
    /// Build the provider. Reads the password from `password_env`; loads
    /// trusted roots eagerly so a broken CA bundle fails at construction,
    /// not mid-send.
    pub fn new(
        host: &str,
        port: u16,
        implicit_tls: bool,
        username: &str,
        password_env: &str,
        ca_bundle_path: Option<&str>,
    ) -> Result<Self, EmailError> {
        let host = host.trim().to_string();
        if host.is_empty() {
            return Err(EmailError::Config("smtp host must not be empty".into()));
        }
        if port == 0 {
            return Err(EmailError::Config("smtp port must not be 0".into()));
        }
        let password = std::env::var(password_env)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| {
                EmailError::Config(format!("env var {password_env} is not set or empty"))
            })?;
        let roots = load_roots(ca_bundle_path)?;

        // Use the process-default crypto provider when installed, otherwise
        // install ring explicitly. This keeps the provider independent of
        // which rustls feature flags other crates enabled.
        let provider = match rustls::crypto::CryptoProvider::get_default() {
            Some(p) => p.clone(),
            None => {
                let provider = rustls::crypto::ring::default_provider();
                Arc::new(provider)
            }
        };
        let config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| EmailError::Config(format!("tls protocol versions: {e}")))?
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(SmtpProvider {
            host,
            port,
            implicit_tls,
            username: username.trim().to_string(),
            password,
            connector: TlsConnector::from(Arc::new(config)),
        })
    }

    async fn deliver(&self, message: &EmailMessage) -> Result<Option<String>, EmailError> {
        let addr = format!("{}:{}", self.host, self.port);
        let tcp = timeout(tokio::net::TcpStream::connect(&addr))
            .await?
            .map_err(|e| EmailError::Transport(format!("tcp connect: {e}")))?;
        let _ = tcp.set_nodelay(true);

        let server_name = rustls::pki_types::ServerName::try_from(self.host.clone())
            .map_err(|_| {
                EmailError::Config(format!("invalid smtp hostname: {}", self.host))
            })?;

        if self.implicit_tls {
            let tls = timeout(self.connector.connect(server_name, tcp))
                .await?
                .map_err(|e| EmailError::TlsRequired(format!("handshake: {e}")))?;
            let mut session = SmtpSession::new(tls);
            self.session(&mut session, message).await
        } else {
            // Plaintext greeting/EHLO, then a MANDATORY STARTTLS upgrade.
            let mut plain = SmtpSession::new(tcp);
            let greeting = plain.read_response().await?;
            require_2xx(&greeting, "greeting")?;
            plain.write_line(&format!("EHLO sniper-suite")).await?;
            let ehlo = plain.read_response().await?;
            require_2xx(&ehlo, "ehlo")?;
            if !response_advertises(&ehlo, "STARTTLS") {
                return Err(EmailError::TlsRequired(
                    "server does not advertise STARTTLS; plaintext sending is disabled"
                        .into(),
                ));
            }
            plain.write_line("STARTTLS").await?;
            let ready = plain.read_response().await?;
            require_2xx(&ready, "starttls")?;
            let tcp = plain.into_inner();
            let tls = timeout(self.connector.connect(server_name, tcp))
                .await?
                .map_err(|e| EmailError::TlsRequired(format!("handshake: {e}")))?;
            let mut session = SmtpSession::new(tls);
            // Re-EHLO after the upgrade, per RFC 3207.
            session.write_line("EHLO sniper-suite").await?;
            let ehlo2 = session.read_response().await?;
            require_2xx(&ehlo2, "ehlo-after-tls")?;
            self.session(&mut session, message).await
        }
    }

    /// AUTH + envelope + DATA on an ESTABLISHED TLS session.
    async fn session<S: AsyncRead + AsyncWrite + Unpin>(
        &self,
        s: &mut SmtpSession<S>,
        message: &EmailMessage,
    ) -> Result<Option<String>, EmailError> {
        s.write_line("AUTH LOGIN").await?;
        let challenge = s.read_response().await?;
        require_code(&challenge, 334, "auth-login")?;
        s.write_line(&base64::engine::general_purpose::STANDARD.encode(self.username.as_bytes()))
            .await?;
        let challenge2 = s.read_response().await?;
        require_code(&challenge2, 334, "auth-username")?;
        s.write_line(&base64::engine::general_purpose::STANDARD.encode(self.password.as_bytes()))
            .await?;
        let authed = s.read_response().await?;
        require_2xx(&authed, "auth-password")?;

        s.write_line(&format!("MAIL FROM:<{}>", message.from)).await?;
        let mail = s.read_response().await?;
        require_2xx(&mail, "mail-from")?;
        s.write_line(&format!("RCPT TO:<{}>", message.to)).await?;
        let rcpt = s.read_response().await?;
        require_2xx(&rcpt, "rcpt-to")?;

        s.write_line("DATA").await?;
        let data = s.read_response().await?;
        require_code(&data, 354, "data")?;

        // Minimal RFC 5322 headers + dot-stuffed body.
        let mut payload = String::new();
        payload.push_str(&format!("From: {}\r\n", message.from));
        payload.push_str(&format!("To: {}\r\n", message.to));
        payload.push_str(&format!("Subject: {}\r\n", message.rendered.subject));
        payload.push_str("MIME-Version: 1.0\r\n");
        payload.push_str("Content-Type: multipart/alternative; boundary=\"snipersuite-boundary\"\r\n");
        payload.push_str("\r\n");
        payload.push_str("--snipersuite-boundary\r\n");
        payload.push_str("Content-Type: text/plain; charset=utf-8\r\n\r\n");
        payload.push_str(&message.rendered.body_text);
        payload.push_str("\r\n--snipersuite-boundary\r\n");
        payload.push_str("Content-Type: text/html; charset=utf-8\r\n\r\n");
        payload.push_str(&message.rendered.body_html);
        payload.push_str("\r\n--snipersuite-boundary--\r\n");

        for line in payload.split('\n') {
            let line = line.trim_end_matches('\r');
            if line.starts_with('.') {
                s.write_line(&format!(".{line}")).await?;
            } else {
                s.write_line(line).await?;
            }
        }
        s.write_line(".").await?;
        let accepted = s.read_response().await?;
        require_2xx(&accepted, "data-accepted")?;

        // Best-effort clean close; failure here does not unsend the mail.
        let _ = s.write_line("QUIT").await;

        // RFC 5321 id when the server gives one (250 2.0.0 Ok: queued as ID).
        let id = accepted
            .split_whitespace()
            .next_back()
            .filter(|w| !w.is_empty() && w.len() < 128)
            .map(str::to_string);
        Ok(id)
    }
}

#[async_trait]
impl EmailProvider for SmtpProvider {
    fn name(&self) -> &'static str {
        "smtp"
    }

    async fn send(&self, message: &EmailMessage) -> Result<Option<String>, EmailError> {
        self.deliver(message).await
    }
}

// ---------------------------------------------------------------------------
// Session plumbing
// ---------------------------------------------------------------------------

/// One line-oriented SMTP session over any async stream.
struct SmtpSession<S: AsyncRead + AsyncWrite + Unpin> {
    io: BufStream<S>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> SmtpSession<S> {
    fn new(stream: S) -> Self {
        SmtpSession {
            io: BufStream::new(stream),
        }
    }

    async fn write_line(&mut self, line: &str) -> Result<(), EmailError> {
        timeout(self.io.write_all(format!("{line}\r\n").as_bytes()))
            .await?
            .map_err(|e| EmailError::Transport(format!("write: {e}")))?;
        timeout(self.io.flush())
            .await?
            .map_err(|e| EmailError::Transport(format!("flush: {e}")))?;
        Ok(())
    }

    /// Read one (possibly multi-line) response. Multi-line responses use
    /// `250-text` continuations and end with `250 text`.
    async fn read_response(&mut self) -> Result<String, EmailError> {
        let mut full = String::new();
        loop {
            let mut line = String::new();
            let n = timeout(self.io.read_line(&mut line))
                .await?
                .map_err(|e| EmailError::Transport(format!("read: {e}")))?;
            if n == 0 {
                return Err(EmailError::Transport("connection closed mid-response".into()));
            }
            let trimmed = line.trim_end_matches(['\r', '\n']);
            full.push_str(trimmed);
            full.push('\n');
            // Continuation lines carry '-' at position 3; the final line a
            // space (or ends at exactly 3 chars).
            let is_last = trimmed.len() < 4 || trimmed.as_bytes()[3] != b'-';
            if is_last {
                return Ok(full);
            }
            if full.len() > 64 * 1024 {
                return Err(EmailError::Transport("oversized smtp response".into()));
            }
        }
    }

    fn into_inner(self) -> S {
        self.io.into_inner()
    }
}

/// Apply the per-phase timeout.
async fn timeout<T>(fut: impl std::future::Future<Output = T>) -> Result<T, EmailError> {
    tokio::time::timeout(PHASE_TIMEOUT, fut)
        .await
        .map_err(|_| EmailError::Timeout)
}

fn require_2xx(response: &str, what: &str) -> Result<(), EmailError> {
    let code = leading_code(response);
    match code {
        Some(c) if (200..300).contains(&c) => Ok(()),
        _ => Err(EmailError::Rejected(format!(
            "{what}: {}",
            first_line(response)
        ))),
    }
}

fn require_code(response: &str, expected: u16, what: &str) -> Result<(), EmailError> {
    match leading_code(response) {
        Some(c) if c == expected => Ok(()),
        _ => Err(EmailError::Rejected(format!(
            "{what}: {}",
            first_line(response)
        ))),
    }
}

fn leading_code(response: &str) -> Option<u16> {
    response.lines().next()?.get(0..3)?.parse().ok()
}

fn first_line(response: &str) -> String {
    response.lines().next().unwrap_or("").chars().take(200).collect()
}

fn response_advertises(ehlo_response: &str, capability: &str) -> bool {
    ehlo_response.lines().any(|line| {
        line.get(4..)
            .map(|rest| rest.eq_ignore_ascii_case(capability))
            .unwrap_or(false)
    })
}

// ---------------------------------------------------------------------------
// Trust roots
// ---------------------------------------------------------------------------

/// Common OS root bundle locations tried when no explicit path is set.
const DEFAULT_CA_PATHS: &[&str] = &[
    "/etc/ssl/certs/ca-certificates.crt",       // Debian/Ubuntu
    "/etc/pki/tls/certs/ca-bundle.crt",         // RHEL/Fedora
    "/etc/ssl/ca-bundle.pem",                   // SUSE
    "/etc/ssl/cert.pem",                        // Alpine/macOS
    "/usr/local/share/certs/ca-root-nss.crt",   // FreeBSD
];

/// Load trusted roots from the configured PEM bundle (or OS defaults).
/// Returns an error naming what was tried — never panics, never trusts
/// anything unparsed.
fn load_roots(ca_bundle_path: Option<&str>) -> Result<rustls::RootCertStore, EmailError> {
    let path = match ca_bundle_path {
        Some(p) if !p.trim().is_empty() => p.trim().to_string(),
        _ => DEFAULT_CA_PATHS
            .iter()
            .find(|p| std::path::Path::new(p).is_file())
            .map(|p| p.to_string())
            .ok_or_else(|| {
                EmailError::Config(
                    "no CA bundle found: set ca_bundle_path (none of the default OS paths exist)"
                        .into(),
                )
            })?,
    };
    let pem = std::fs::read_to_string(&path)
        .map_err(|e| EmailError::Config(format!("cannot read CA bundle {path}: {e}")))?;
    let ders = pem_to_der(&pem);
    if ders.is_empty() {
        return Err(EmailError::Config(format!(
            "CA bundle {path} contains no certificates"
        )));
    }
    let mut roots = rustls::RootCertStore::empty();
    let (added, _ignored) = roots.add_parsable_certificates(
        ders.into_iter().map(rustls::pki_types::CertificateDer::from),
    );
    if added == 0 {
        return Err(EmailError::Config(format!(
            "no certificates in {path} parsed as valid trust roots"
        )));
    }
    Ok(roots)
}

/// Extract DER certificates from PEM text (hand-rolled to avoid depending
/// on a pemfile crate; malformed blocks are skipped, not fatal).
fn pem_to_der(pem: &str) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_block = false;
    for line in pem.lines() {
        let line = line.trim();
        if line == "-----BEGIN CERTIFICATE-----" {
            current.clear();
            in_block = true;
        } else if line == "-----END CERTIFICATE-----" {
            if in_block {
                if let Ok(der) =
                    base64::engine::general_purpose::STANDARD.decode(current.trim())
                {
                    out.push(der);
                }
            }
            in_block = false;
        } else if in_block {
            current.push_str(line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pem_parsing_extracts_certificates_and_skips_garbage() {
        // A syntactically complete (but semantically dummy) certificate
        // block plus junk around it.
        let pem = "junk\n-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n\
                   -----BEGIN CERTIFICATE-----\n!!!not-base64!!!\n-----END CERTIFICATE-----\n";
        let ders = pem_to_der(pem);
        assert_eq!(ders.len(), 1, "one valid block parsed, invalid skipped");
        assert_eq!(ders[0], vec![1, 2, 3]);
        assert!(pem_to_der("no blocks here").is_empty());
    }

    #[test]
    fn response_helpers_parse_codes_and_capabilities() {
        assert_eq!(leading_code("250 Ok\n"), Some(250));
        assert_eq!(leading_code("550-Nope\n550 really not"), Some(550));
        assert!(require_2xx("250 2.1.0 Ok\n", "x").is_ok());
        assert!(require_2xx("451 try later\n", "x").is_err());
        assert!(require_code("334 VXNlcm5hbWU6\n", 334, "x").is_ok());
        let ehlo = "250-mail.example.com\n250-STARTTLS\n250-AUTH LOGIN PLAIN\n250 8BITMIME\n";
        assert!(response_advertises(ehlo, "STARTTLS"));
        assert!(!response_advertises(ehlo, "PIPELINING"));
        // EHLO keywords are case-insensitive (RFC 5321 section 4.1.1.1), so a
        // lowercase `starttls` line does advertise STARTTLS.
        assert!(response_advertises("250-mail\n250 starttls", "STARTTLS"));
    }

    #[test]
    fn provider_construction_validates_inputs() {
        std::env::remove_var("TEST_SMTP_PW_ABSENT");
        let e = SmtpProvider::new("mail.example.com", 587, false, "user", "TEST_SMTP_PW_ABSENT", None)
            .unwrap_err();
        assert!(matches!(e, EmailError::Config(_)), "missing password env");
        std::env::set_var("TEST_SMTP_PW_PRESENT", "secret");
        let e = SmtpProvider::new("", 587, false, "user", "TEST_SMTP_PW_PRESENT", None).unwrap_err();
        assert!(matches!(e, EmailError::Config(_)), "empty host");
        let e = SmtpProvider::new("mail.example.com", 0, false, "user", "TEST_SMTP_PW_PRESENT", None)
            .unwrap_err();
        assert!(matches!(e, EmailError::Config(_)), "zero port");
        // A nonexistent explicit CA path is a Config error at build time.
        let e = SmtpProvider::new(
            "mail.example.com",
            587,
            false,
            "user",
            "TEST_SMTP_PW_PRESENT",
            Some("/nonexistent/bundle.pem"),
        )
        .unwrap_err();
        assert!(matches!(e, EmailError::Config(_)));
        std::env::remove_var("TEST_SMTP_PW_PRESENT");
    }
}
