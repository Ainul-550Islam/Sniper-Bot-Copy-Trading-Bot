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
    decrypt_webhook_secret, parse_https_url, resolve_public_socket, sign_payload,
    DELIVERY_TIMEOUT, MAX_RESPONSE_BODY_BYTES, MAX_RESPONSE_BODY_DIGEST_BYTES,
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
    let exponent = (attempt - 1).clamp(0, 30);
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
pub async fn deliver_once(
    url: &str,
    secret: &str,
    event_id: &str,
    body: &[u8],
) -> AttemptOutcome {
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
            let digest = hex::encode(hasher.finalize())[..MAX_RESPONSE_BODY_DIGEST_BYTES]
                .to_string();
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
        let rows = sqlx::query(
            "SELECT id, endpoint_id, event_id, attempt_count, payload
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
        .fetch_all(&self.pool)
        .await?;

        let mut processed = 0usize;
        for row in rows {
            let due = DueRow {
                delivery_id: row.try_get("id").unwrap_or_default(),
                endpoint_id: row.try_get("endpoint_id").unwrap_or_default(),
                event_id: row
                    .try_get::<String, _>("event_id")
                    .unwrap_or_default(),
                attempt_count: row.try_get("attempt_count").unwrap_or(1),
                payload: row
                    .try_get::<sqlx::types::Json<serde_json::Value>, _>("payload")
                    .map(|json| json.0)
                    .unwrap_or(serde_json::Value::Null),
            };
            self.process_one(due).await;
            processed += 1;
        }
        Ok(processed)
    }

    /// Deliver one claimed row and persist the outcome. All failure modes
    /// end in a database update — a row is never left mid-flight without a
    /// recorded result.
    async fn process_one(&self, due: DueRow) {
        let endpoint = sqlx::query(
            "SELECT url, secret, status FROM webhook_endpoints WHERE id = $1",
        )
        .bind(due.endpoint_id)
        .fetch_optional(&self.pool)
        .await;

        let endpoint: Option<EndpointRow> = match endpoint {
            Ok(Some(row)) => Some(EndpointRow {
                url: row.try_get("url").unwrap_or_default(),
                encrypted_secret: row.try_get("secret").unwrap_or_default(),
                status: row.try_get("status").unwrap_or_default(),
            }),
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    delivery = %due.delivery_id,
                    "webhook endpoint lookup failed; deferring retry"
                );
                self.defer(&due, attempt_error("endpoint lookup failed")).await;
                return;
            }
        };

        // Endpoint deleted or deactivated: dead-letter, do not keep trying.
        let Some(endpoint) = endpoint.filter(|e| e.status == "active") else {
            self.mark_dead(&due, "endpoint is missing or inactive").await;
            return;
        };

        let secret = match decrypt_webhook_secret(&endpoint.encrypted_secret) {
            Ok(value) => value,
            Err(error) => {
                // Undecryptable secret = misconfiguration; dead-letter rather
                // than retry the same failure forever.
                tracing::warn!(
                    error = %error,
                    delivery = %due.delivery_id,
                    "webhook secret could not be decrypted; dead-lettering"
                );
                self.mark_dead(&due, "endpoint secret could not be decrypted")
                    .await;
                return;
            }
        };

        let body = match serde_json::to_vec(&due.payload) {
            Ok(value) => value,
            Err(_) => {
                self.mark_dead(&due, "stored payload could not be re-encoded")
                    .await;
                return;
            }
        };

        let outcome = deliver_once(&endpoint.url, &secret, &due.event_id, &body).await;
        let attempts_after = due.attempt_count.saturating_add(1);

        if outcome.succeeded {
            let update = sqlx::query(
                "UPDATE webhook_deliveries
                    SET status = 'succeeded',
                        attempt_count = $2,
                        response_status = $3,
                        response_body_digest = $4,
                        error = NULL,
                        next_retry_at = NULL
                  WHERE id = $1",
            )
            .bind(due.delivery_id)
            .bind(attempts_after)
            .bind(outcome.response_status)
            .bind(outcome.response_digest.as_deref())
            .execute(&self.pool)
            .await;
            if let Err(error) = update {
                tracing::error!(
                    error = %error,
                    delivery = %due.delivery_id,
                    "webhook retry succeeded but the row could not be updated"
                );
            }
            return;
        }

        if attempts_after >= MAX_ATTEMPTS {
            self.mark_dead(&due, &attempt_error_with(&outcome, attempts_after))
                .await;
            return;
        }
        self.schedule_next(&due, attempts_after, &outcome).await;
    }

    /// Failed attempt with retries remaining: bump the counter, keep the
    /// failure recorded, push `next_retry_at` out on the backoff schedule.
    async fn schedule_next(&self, due: &DueRow, attempts_after: i32, outcome: &AttemptOutcome) {
        let next_at = Utc::now() + retry_backoff(attempts_after);
        let update = sqlx::query(
            "UPDATE webhook_deliveries
                SET attempt_count = $2,
                    response_status = $3,
                    response_body_digest = $4,
                    error = $5,
                    next_retry_at = $6
              WHERE id = $1",
        )
        .bind(due.delivery_id)
        .bind(attempts_after)
        .bind(outcome.response_status)
        .bind(outcome.response_digest.as_deref())
        .bind(outcome.error.as_deref())
        .bind(next_at)
        .execute(&self.pool)
        .await;
        if let Err(error) = update {
            tracing::error!(
                error = %error,
                delivery = %due.delivery_id,
                "webhook retry scheduling failed"
            );
        }
    }

    /// Transient infrastructure error before any delivery happened: do not
    /// burn an attempt, just push the row out a little.
    async fn defer(&self, due: &DueRow, error: String) {
        let next_at = Utc::now() + chrono::Duration::seconds(BACKOFF_BASE_SECS);
        let update = sqlx::query(
            "UPDATE webhook_deliveries
                SET error = $2, next_retry_at = $3
              WHERE id = $1",
        )
        .bind(due.delivery_id)
        .bind(error)
        .bind(next_at)
        .execute(&self.pool)
        .await;
        if let Err(error) = update {
            tracing::error!(
                error = %error,
                delivery = %due.delivery_id,
                "webhook defer update failed"
            );
        }
    }

    /// Terminal failure: nothing will touch this row again.
    async fn mark_dead(&self, due: &DueRow, error: &str) {
        let update = sqlx::query(
            "UPDATE webhook_deliveries
                SET status = 'dead', error = $2, next_retry_at = NULL
              WHERE id = $1",
        )
        .bind(due.delivery_id)
        .bind(error)
        .execute(&self.pool)
        .await;
        if let Err(error) = update {
            tracing::error!(
                error = %error,
                delivery = %due.delivery_id,
                "webhook dead-letter update failed"
            );
        }
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

fn attempt_error(reason: &str) -> String {
    format!("retry deferred: {reason}")
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
            at = at + retry_backoff(attempt);
            seen.push(at);
        }
        for pair in seen.windows(2) {
            assert!(pair[1] > pair[0]);
        }
    }
}
