//! Durable webhook delivery retries (GAP-MAP v2 P1 — closes the
//! "no retry / dead-letter loop" finding on `webhooks.rs`).
//!
//! # Model
//!
//! Every delivery attempt is a row in `webhook_deliveries`
//! (PostgreSQL-authoritative). A row that fails carries `next_retry_at`;
//! this dispatcher wakes on a tick, claims due rows with
//! `FOR UPDATE SKIP LOCKED` (multi-replica safe — two dispatchers never
//! double-deliver the same row), re-delivers the STORED payload
//! byte-identically (re-signed with a fresh timestamp), and either marks
//! the row `succeeded` or schedules the next attempt.
//!
//! * Backoff: `retry_backoff(attempt)` = 60 s · 2^(attempt−1), capped at
//!   30 minutes. Attempt 1 fails → retry in 60 s; attempt 2 → 2 min; etc.
//! * Dead letter: after [`MAX_ATTEMPTS`] failures the row becomes `dead`
//!   and is never touched again. Tenants see it in the delivery journal.
//! * Endpoint gone or inactive → the row becomes `dead` immediately
//!   (nothing to deliver to; keeping it `failed` would retry forever).
//!
//! # Safety properties reused from `webhooks.rs`
//!
//! The single-attempt path is the SAME code shape as the synchronous test
//! delivery: HTTPS-only, no redirects, DNS resolved once and pinned (no
//! TOCTOU between check and connect), private/link-local destinations
//! refused, response bodies digested under a size cap. Secrets stay
//! encrypted at rest and are decrypted only at signing time.

use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::Utc;
use futures::StreamExt;
use reqwest::redirect::Policy;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use bot_core::lifecycle::Shutdown;

use super::webhooks::{
    decrypt_webhook_secret, parse_https_url, resolve_public_socket, sign_payload, DELIVERY_TIMEOUT,
    MAX_RESPONSE_BODY_BYTES, MAX_RESPONSE_BODY_DIGEST_BYTES,
};

/// Total attempts per delivery (the initial one + retries). After this the
/// row is dead-lettered.
pub const MAX_ATTEMPTS: i32 = 8;

/// Base of the exponential backoff schedule (seconds).
const BACKOFF_BASE_SECS: i64 = 60;

/// Never wait longer than this between attempts (seconds).
const BACKOFF_CAP_SECS: i64 = 30 * 60;

/// Rows claimed per tick — keeps one tick bounded.
const BATCH_LIMIT: i64 = 25;

/// How often the dispatcher looks for due rows.
const DEFAULT_POLL: StdDuration = StdDuration::from_secs(15);

/// Exponential backoff for the retry AFTER attempt `attempt` failed
/// (1-based). Monotone, capped, and safe for large inputs.
pub fn retry_backoff(attempt: i32) -> chrono::Duration {
    let exponent = attempt.saturating_sub(1).clamp(0, 30);
    let seconds = BACKOFF_BASE_SECS.saturating_mul(1i64 << exponent.min(20));
    let capped = seconds.min(BACKOFF_CAP_SECS);
    chrono::Duration::seconds(capped)
}

/// The outcome of one single-attempt delivery.
pub struct AttemptOutcome {
    pub succeeded: bool,
    pub response_status: Option<i32>,
    pub response_digest: Option<String>,
    pub error: Option<String>,
}

/// Deliver one payload once, with the full SSRF/redirect/timeout guard set.
///
/// Returns an [`AttemptOutcome`]; never panics. Transport-level failures
/// and non-2xx responses are both `succeeded == false` with a short,
/// log-safe `error` string.
pub async fn deliver_once(url: &str, secret: &str, event_id: &str, body: &[u8]) -> AttemptOutcome {
    let parsed = match parse_https_url(url) {
        Ok(value) => value,
        Err(reason) => {
            return AttemptOutcome {
                succeeded: false,
                response_status: None,
                response_digest: None,
                error: Some(format!("invalid webhook url: {reason}")),
            }
        }
    };
    let socket = match resolve_public_socket(&parsed).await {
        Ok(value) => value,
        Err(reason) => {
            return AttemptOutcome {
                succeeded: false,
                response_status: None,
                response_digest: None,
                error: Some(format!("webhook host unavailable: {reason}")),
            }
        }
    };
    let host = parsed.host_str().unwrap_or_default().to_string();
    let client = match reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(DELIVERY_TIMEOUT)
        .resolve(&host, socket)
        .build()
    {
        Ok(value) => value,
        Err(error) => {
            return AttemptOutcome {
                succeeded: false,
                response_status: None,
                response_digest: None,
                error: Some(format!("webhook client error: {error}")),
            }
        }
    };

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let signature = sign_payload(secret, timestamp, body);

    let send = client
        .post(url)
        .header("content-type", "application/json")
        .header("user-agent", "sniper-suite-webhook/1")
        .header("x-webhook-id", event_id)
        .header("x-webhook-timestamp", timestamp.to_string())
        .header("x-webhook-signature", format!("v1={signature}"))
        .body(body.to_vec())
        .send()
        .await;

    match send {
        Ok(response) => {
            let code = response.status().as_u16() as i32;
            // Digest the body under the size cap without buffering it.
            let mut stream = response.bytes_stream();
            let mut hasher = Sha256::new();
            let mut total = 0usize;
            let mut read_error = None;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(bytes) => {
                        let remaining = MAX_RESPONSE_BODY_BYTES.saturating_sub(total);
                        let accepted = bytes.len().min(remaining);
                        hasher.update(&bytes[..accepted]);
                        total = total.saturating_add(accepted);
                        if accepted < bytes.len() {
                            break;
                        }
                    }
                    Err(_) => {
                        read_error = Some("webhook response body could not be read".to_string());
                        break;
                    }
                }
            }
            let succeeded = (200..300).contains(&code) && read_error.is_none();
            let error = if let Some(reason) = read_error {
                Some(reason)
            } else if !succeeded {
                Some(format!("remote endpoint returned HTTP {code}"))
            } else {
                None
            };
            let digest =
                hex::encode(hasher.finalize())[..MAX_RESPONSE_BODY_DIGEST_BYTES].to_string();
            AttemptOutcome {
                succeeded,
                response_status: Some(code),
                response_digest: Some(digest),
                error,
            }
        }
        Err(error) => AttemptOutcome {
            succeeded: false,
            response_status: None,
            response_digest: None,
            error: Some(if error.is_timeout() {
                "webhook delivery timed out".to_string()
            } else {
                "webhook delivery failed".to_string()
            }),
        },
    }
}

/// The dispatcher. One per process is enough (`SKIP LOCKED` makes extra
/// instances harmless), and `run` stops on [`Shutdown`].
pub struct WebhookRetryDispatcher {
    pool: PgPool,
    poll: StdDuration,
}

/// One due delivery row as claimed from the scan.
struct DueRow {
    delivery_id: Uuid,
    organization_id: Uuid,
    endpoint_id: Uuid,
    event_id: String,
    attempt_count: i32,
    payload: serde_json::Value,
}

/// One endpoint row needed to (re-)deliver.
struct EndpointRow {
    url: String,
    encrypted_secret: String,
    status: String,
}

impl WebhookRetryDispatcher {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            poll: DEFAULT_POLL,
        }
    }

    pub fn with_poll_interval(mut self, poll: StdDuration) -> Self {
        self.poll = poll.max(StdDuration::from_secs(1));
        self
    }

    /// One full pass: claim due rows, deliver, record. Returns the number
    /// of rows processed (0 is a normal idle tick).
    pub async fn run_once(&self) -> Result<usize, sqlx::Error> {
        // SKIP LOCKED only protects a claim while its transaction is alive.
        // Keep those row locks through delivery and persistence, so another
        // dispatcher cannot send the same attempt concurrently.
        let mut transaction = self.pool.begin().await?;
        let rows = sqlx::query(
            "SELECT id, organization_id, endpoint_id, event_id, attempt_count, payload
               FROM webhook_deliveries
              WHERE status = 'failed'
                AND attempt_count < $2
                AND next_retry_at IS NOT NULL
                AND next_retry_at <= now()
              ORDER BY next_retry_at
              LIMIT $1
              FOR UPDATE SKIP LOCKED",
        )
        .bind(BATCH_LIMIT)
        .bind(MAX_ATTEMPTS)
        .fetch_all(&mut *transaction)
        .await?;

        let mut processed = 0usize;
        for row in rows {
            let due = DueRow {
                delivery_id: row.try_get("id")?,
                organization_id: row.try_get("organization_id")?,
                endpoint_id: row.try_get("endpoint_id")?,
                event_id: row.try_get("event_id")?,
                attempt_count: row.try_get("attempt_count")?,
                payload: row
                    .try_get::<sqlx::types::Json<serde_json::Value>, _>("payload")?
                    .0,
            };
            self.process_one(&mut transaction, due).await?;
            processed += 1;
        }
        transaction.commit().await?;
        Ok(processed)
    }

    /// Deliver and persist under the same claim lock. Database failures are
    /// returned to the caller, not counted as successfully processed rows.
    async fn process_one(
        &self,
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        mut due: DueRow,
    ) -> Result<(), sqlx::Error> {
        let endpoint = sqlx::query(
            "SELECT url, secret, status FROM webhook_endpoints WHERE id = $1 AND organization_id = $2",
        )
        .bind(due.endpoint_id)
        .bind(due.organization_id)
        .fetch_optional(&mut **transaction)
        .await?;
        let endpoint = endpoint
            .map(|row| -> Result<EndpointRow, sqlx::Error> {
                Ok(EndpointRow {
                    url: row.try_get("url")?,
                    encrypted_secret: row.try_get("secret")?,
                    status: row.try_get("status")?,
                })
            })
            .transpose()?;
        let Some(endpoint) = endpoint.filter(|endpoint| endpoint.status == "active") else {
            return self
                .mark_dead(transaction, &due, "endpoint is missing or inactive")
                .await;
        };
        let secret = match decrypt_webhook_secret(&endpoint.encrypted_secret) {
            Ok(secret) => secret,
            Err(error) => {
                tracing::warn!(error = %error, delivery = %due.delivery_id,
                    "webhook secret could not be decrypted; dead-lettering");
                return self
                    .mark_dead(transaction, &due, "endpoint secret could not be decrypted")
                    .await;
            }
        };
        let body = match serde_json::to_vec(&due.payload) {
            Ok(body) => body,
            Err(_) => {
                return self
                    .mark_dead(transaction, &due, "stored payload could not be re-encoded")
                    .await
            }
        };
        let outcome = deliver_once(&endpoint.url, &secret, &due.event_id, &body).await;
        due.attempt_count = due.attempt_count.saturating_add(1);
        let status = if outcome.succeeded {
            "succeeded"
        } else if due.attempt_count >= MAX_ATTEMPTS {
            "dead"
        } else {
            "failed"
        };
        let next_at = (status == "failed").then(|| Utc::now() + retry_backoff(due.attempt_count));
        let error = if status == "dead" {
            Some(attempt_error_with(&outcome, due.attempt_count))
        } else {
            outcome.error
        };
        // The final failed attempt counts too; dead-lettering must not leave
        // an exhausted row at MAX_ATTEMPTS - 1.
        sqlx::query(
            "UPDATE webhook_deliveries
                SET status = $3, attempt_count = $4, response_status = $5,
                    response_body_digest = $6, error = $7, next_retry_at = $8
              WHERE id = $1 AND organization_id = $2",
        )
        .bind(due.delivery_id)
        .bind(due.organization_id)
        .bind(status)
        .bind(due.attempt_count)
        .bind(outcome.response_status)
        .bind(outcome.response_digest.as_deref())
        .bind(error.as_deref())
        .bind(next_at)
        .execute(&mut **transaction)
        .await?;
        Ok(())
    }

    /// Configuration failures before a send are terminal, but do not claim
    /// that a network attempt happened. Plaintext secrets are never accepted.
    async fn mark_dead(
        &self,
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        due: &DueRow,
        error: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE webhook_deliveries SET status = 'dead', error = $3, next_retry_at = NULL
              WHERE id = $1 AND organization_id = $2",
        )
        .bind(due.delivery_id)
        .bind(due.organization_id)
        .bind(error)
        .execute(&mut **transaction)
        .await?;
        Ok(())
    }

    /// Run until shutdown. Never panics; a failing tick logs and retries on
    /// the next one.
    pub async fn run(self: Arc<Self>, shutdown: Arc<Shutdown>) {
        let mut ticker = tokio::time::interval(self.poll);
        ticker.tick().await; // first tick is immediate — skip it
        loop {
            tokio::select! {
                _ = shutdown.wait() => break,
                _ = ticker.tick() => {
                    match self.run_once().await {
                        Ok(0) => {}
                        Ok(n) => tracing::debug!(processed = n, "webhook retry pass"),
                        Err(error) => {
                            tracing::warn!(error = %error, "webhook retry pass failed");
                        }
                    }
                }
            }
        }
        tracing::info!("webhook retry dispatcher stopped");
    }
}

fn attempt_error_with(outcome: &AttemptOutcome, attempts_after: i32) -> String {
    format!(
        "gave up after {attempts_after} attempts: {}",
        outcome
            .error
            .clone()
            .unwrap_or_else(|| "webhook delivery failed".to_string())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_exponential_and_capped() {
        assert_eq!(retry_backoff(1).num_seconds(), 60);
        assert_eq!(retry_backoff(2).num_seconds(), 120);
        assert_eq!(retry_backoff(3).num_seconds(), 240);
        assert_eq!(retry_backoff(4).num_seconds(), 480);
        assert_eq!(retry_backoff(5).num_seconds(), 960);
        // Capped at 30 minutes from attempt 6 onward.
        assert_eq!(retry_backoff(6).num_seconds(), 1800);
        assert_eq!(retry_backoff(7).num_seconds(), 1800);
        assert_eq!(retry_backoff(MAX_ATTEMPTS).num_seconds(), 1800);
    }

    #[test]
    fn backoff_never_panics_on_degenerate_inputs() {
        assert!(retry_backoff(0).num_seconds() >= 60);
        assert!(retry_backoff(-5).num_seconds() >= 60);
        assert!(retry_backoff(i32::MAX).num_seconds() <= 1800);
    }

    #[test]
    fn backoff_is_monotone() {
        let mut previous = 0i64;
        for attempt in 1..=MAX_ATTEMPTS {
            let seconds = retry_backoff(attempt).num_seconds();
            assert!(seconds >= previous, "attempt {attempt}");
            previous = seconds;
        }
    }

    #[test]
    fn schedule_covers_the_full_retry_budget() {
        // Initial attempt + (MAX_ATTEMPTS - 1) retries, each strictly
        // later than the one before it until the cap.
        let mut at = Utc::now();
        let mut seen = vec![at];
        for attempt in 1..MAX_ATTEMPTS {
            at += retry_backoff(attempt);
            seen.push(at);
        }
        for pair in seen.windows(2) {
            assert!(pair[1] > pair[0]);
        }
    }
}
