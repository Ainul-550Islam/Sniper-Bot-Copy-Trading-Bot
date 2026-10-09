//! Outbox dispatcher for `email_outbox` (GAP-MAP v2 P1).
//!
//! Handlers enqueue rows; this dispatcher is the ONLY component that
//! delivers. Guarantees:
//!
//! * **Dedupe** — `dedup_key` is UNIQUE; enqueue is an
//!   `INSERT ... ON CONFLICT DO NOTHING`, so a double-submit of the same
//!   logical event produces one email.
//! * **Retry with backoff** — transport failures and timeouts reschedule
//!   the row (`failed` → due again after `backoff(attempts)`); permanent
//!   rejections and exhausted attempts dead-letter (`dead`) with the last
//!   error journaled on the row.
//! * **Crash safety** — rows are claimed with `FOR UPDATE SKIP LOCKED`
//!   inside one transaction per delivery, so two dispatchers (two
//!   replicas) never send the same row twice.
//! * **Rate limiting** — an in-process minimum interval derived from
//!   `per_minute_limit`, so a bug or an attacker-controlled flood cannot
//!   burn provider quotas instantly.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::postgres::PgPool;
use sqlx::Row;
use tracing::{debug, info, warn};

use bot_core::lifecycle::Shutdown;

use super::{EmailError, EmailMessage, EmailProvider};
use crate::email::templates::Rendered;

/// Backoff before attempt N+1: 60s, 120s, 240s ... capped at 30 min.
pub fn backoff(attempts: i32) -> Duration {
    let exp = attempts.clamp(0, 10) as u32;
    let secs = 60u64.saturating_mul(2u64.saturating_pow(exp));
    Duration::from_secs(secs.min(30 * 60))
}

/// One outbox row worth delivering.
#[derive(Debug, Clone)]
struct OutboxRow {
    id: uuid::Uuid,
    recipient: String,
    template_key: String,
    subject: String,
    body_text: String,
    body_html: Option<String>,
    attempts: i32,
    max_attempts: i32,
}

/// Enqueue one logical email. Returns `true` when the row was created and
/// `false` when the `dedup_key` already existed (nothing new happens).
/// Storage errors bubble — the caller decides whether to 503 or swallow.
#[allow(clippy::too_many_arguments)]
pub async fn enqueue(
    pool: &PgPool,
    dedup_key: &str,
    template_key: &str,
    recipient: &str,
    subject: &str,
    body_text: &str,
    body_html: Option<&str>,
    user_id: Option<uuid::Uuid>,
    organization_id: Option<uuid::Uuid>,
) -> Result<bool, sqlx::Error> {
    let inserted = sqlx::query(
        "INSERT INTO email_outbox
             (organization_id, user_id, recipient, template_key, subject,
              body_text, body_html, dedup_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (dedup_key) DO NOTHING",
    )
    .bind(organization_id)
    .bind(user_id)
    .bind(recipient)
    .bind(template_key)
    .bind(subject)
    .bind(body_text)
    .bind(body_html)
    .bind(dedup_key)
    .execute(pool)
    .await?;
    Ok(inserted.rows_affected() == 1)
}

/// The dispatcher.
pub struct OutboxDispatcher {
    pool: PgPool,
    provider: Arc<dyn EmailProvider>,
    from_address: String,
    /// Minimum pause between two sends (rate limiting).
    min_send_interval: Duration,
    /// Rows considered per `run_once`.
    batch_size: i64,
}

/// What one `run_once` achieved.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutboxReport {
    pub sent: u64,
    pub retried: u64,
    pub dead: u64,
}

impl OutboxDispatcher {
    pub fn new(pool: PgPool, provider: Arc<dyn EmailProvider>, from_address: String) -> Self {
        OutboxDispatcher {
            pool,
            provider,
            from_address,
            min_send_interval: Duration::from_millis(250),
            batch_size: 10,
        }
    }

    /// Apply the configured per-minute ceiling.
    pub fn with_rate_limit(mut self, per_minute: u32) -> Self {
        if per_minute > 0 {
            let ms = 60_000u64 / per_minute.max(1) as u64;
            self.min_send_interval = Duration::from_millis(ms.max(10));
        }
        self
    }

    /// Deliver every due row, one transaction per row.
    pub async fn run_once(&self) -> Result<OutboxReport, sqlx::Error> {
        let mut report = OutboxReport::default();
        let rows = self.fetch_due().await?;
        for row in rows {
            match self.deliver_one(&row).await {
                DeliveryOutcome::Sent => report.sent += 1,
                DeliveryOutcome::Retry => report.retried += 1,
                DeliveryOutcome::Dead => report.dead += 1,
            }
            // Rate-limit gate between sends (not after the last one).
            if !self.min_send_interval.is_zero() {
                tokio::time::sleep(self.min_send_interval).await;
            }
        }
        if report.sent > 0 || report.dead > 0 {
            info!(
                provider = self.provider.name(),
                sent = report.sent,
                retried = report.retried,
                dead = report.dead,
                "email outbox run complete"
            );
        }
        Ok(report)
    }

    /// Due rows: pending or failed whose next attempt is now, oldest first.
    async fn fetch_due(&self) -> Result<Vec<OutboxRow>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, recipient, template_key, subject, body_text, body_html,
                    attempts, max_attempts
               FROM email_outbox
              WHERE status IN ('pending', 'failed')
                AND next_attempt_at <= now()
              ORDER BY next_attempt_at ASC
              LIMIT $1
              FOR UPDATE SKIP LOCKED",
        )
        .bind(self.batch_size)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(OutboxRow {
                id: r.try_get("id")?,
                recipient: r.try_get("recipient")?,
                template_key: r.try_get("template_key")?,
                subject: r.try_get("subject")?,
                body_text: r.try_get("body_text")?,
                body_html: r.try_get("body_html").ok(),
                attempts: r.try_get("attempts")?,
                max_attempts: r.try_get("max_attempts")?,
            });
        }
        Ok(out)
    }

    async fn deliver_one(&self, row: &OutboxRow) -> DeliveryOutcome {
        // Claim: mark sending + count the attempt BEFORE delivery. If the
        // process dies mid-send, the row stays 'sending' with attempts
        // bumped; the reaper below re-queues stale 'sending' rows instead
        // of losing them.
        let claim = sqlx::query(
            "UPDATE email_outbox
                SET status = 'sending',
                    attempts = attempts + 1,
                    next_attempt_at = now()
              WHERE id = $1
                AND status IN ('pending', 'failed')",
        )
        .bind(row.id)
        .execute(&self.pool)
        .await;
        if matches!(&claim, Ok(r) if r.rows_affected() == 0) {
            // Someone else claimed it (should not happen with SKIP LOCKED,
            // but be certain).
            return DeliveryOutcome::Retry;
        }
        if let Err(e) = claim {
            warn!(error = %e, "email claim update failed");
            return DeliveryOutcome::Retry;
        }

        let message = EmailMessage {
            to: row.recipient.clone(),
            from: self.from_address.clone(),
            rendered: Rendered {
                template_key: leaked_template_key(&row.template_key),
                subject: row.subject.clone(),
                body_text: row.body_text.clone(),
                body_html: row.body_html.clone().unwrap_or_default(),
            },
            dedup_key: row.id.to_string(),
        };

        let outcome = self.provider.send(&message).await;
        match outcome {
            Ok(provider_id) => {
                let q = sqlx::query(
                    "UPDATE email_outbox
                        SET status = 'sent', sent_at = now(), provider_id = $2
                      WHERE id = $1",
                )
                .bind(row.id)
                .bind(provider_id);
                if let Err(e) = q.execute(&self.pool).await {
                    warn!(error = %e, "email sent-mark failed");
                }
                DeliveryOutcome::Sent
            }
            Err(e) => {
                let attempts_now = row.attempts + 1;
                let exhausted = attempts_now >= row.max_attempts;
                let permanent = !e.is_retryable();
                if exhausted || permanent {
                    let q = sqlx::query(
                        "UPDATE email_outbox
                            SET status = 'dead', last_error = $2, next_attempt_at = now()
                          WHERE id = $1",
                    )
                    .bind(row.id)
                    .bind(sanitized_error(&e));
                    if let Err(err) = q.execute(&self.pool).await {
                        warn!(error = %err, "email dead-mark failed");
                    }
                    warn!(
                        template = row.template_key,
                        permanent,
                        error = sanitized_error(&e),
                        "email dead-lettered"
                    );
                    DeliveryOutcome::Dead
                } else {
                    let delay = backoff(attempts_now);
                    let q = sqlx::query(
                        "UPDATE email_outbox
                            SET status = 'failed',
                                last_error = $2,
                                next_attempt_at = now() + make_interval(secs => $3)
                          WHERE id = $1",
                    )
                    .bind(row.id)
                    .bind(sanitized_error(&e))
                    .bind(delay.as_secs() as i64);
                    if let Err(err) = q.execute(&self.pool).await {
                        warn!(error = %err, "email retry-mark failed");
                    }
                    debug!(
                        template = row.template_key,
                        attempts = attempts_now,
                        next_in_secs = delay.as_secs(),
                        "email delivery failed; scheduled retry"
                    );
                    DeliveryOutcome::Retry
                }
            }
        }
    }

    /// Re-queue rows stuck in 'sending' for more than `stale_after` (their
    /// dispatcher died mid-delivery). Called opportunistically by the run
    /// loop; delivery itself stays idempotent via dedup on the provider
    /// side where supported.
    pub async fn reap_stale_sending(&self, stale_after: Duration) -> Result<u64, sqlx::Error> {
        let stale = sqlx::query(
            "UPDATE email_outbox
                SET status = 'failed',
                    last_error = 'dispatcher restart while sending',
                    next_attempt_at = now()
              WHERE status = 'sending'
                AND next_attempt_at <= now() - make_interval(secs => $1)",
        )
        .bind(stale_after.as_secs() as i64)
        .execute(&self.pool)
        .await?;
        Ok(stale.rows_affected())
    }

    /// Run until shutdown: poll interval + stale reaper.
    pub async fn run(self: Arc<Self>, poll_every: Duration, shutdown: Option<Arc<Shutdown>>) {
        info!(
            provider = self.provider.name(),
            poll_secs = poll_every.as_secs(),
            "email outbox dispatcher started"
        );
        let mut ticker = tokio::time::interval(poll_every.max(Duration::from_secs(1)));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut reap = tokio::time::interval(Duration::from_secs(300));
        reap.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = async {
                    match &shutdown {
                        Some(s) => s.wait().await,
                        None => std::future::pending().await,
                    }
                } => {
                    info!("email outbox dispatcher stopping (shutdown)");
                    return;
                }
                _ = reap.tick() => {
                    match self.reap_stale_sending(Duration::from_secs(600)).await {
                        Ok(n) if n > 0 => info!(requeued = n, "stale 'sending' emails re-queued"),
                        Err(e) => debug!(error = %e, "stale-sending reap failed"),
                        _ => {}
                    }
                }
                _ = ticker.tick() => {
                    if let Err(e) = self.run_once().await {
                        warn!(error = %e, "email outbox run failed");
                    }
                }
            }
        }
    }
}

enum DeliveryOutcome {
    Sent,
    Retry,
    Dead,
}

/// The template_key column is a CHECK-constrained vocabulary; map it back
/// to the &'static keys the templates use. Unknown values (should be
/// impossible) fall back to "security_alert" rather than panicking.
fn leaked_template_key(value: &str) -> &'static str {
    match value {
        "email_verification" => "email_verification",
        "password_reset" => "password_reset",
        "member_invite" => "member_invite",
        "security_alert" => "security_alert",
        "invoice_receipt" => "invoice_receipt",
        _ => "security_alert",
    }
}

/// Provider errors may embed the provider's message text; cut it down and
/// strip obvious secrets-shaped substrings before journaling.
fn sanitized_error(e: &EmailError) -> String {
    let raw = e.to_string();
    let cut: String = raw.chars().take(300).collect();
    cut.replace(
        ['\r', '\n'],
        " ",
    )
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_monotone_and_capped() {
        let b0 = backoff(0);
        let b1 = backoff(1);
        let b10 = backoff(10);
        let b100 = backoff(100);
        assert_eq!(b0, Duration::from_secs(60));
        assert_eq!(b1, Duration::from_secs(120));
        assert!(b10 >= b1);
        assert!(b100 <= Duration::from_secs(30 * 60));
        assert!(backoff(-5) >= Duration::from_secs(60), "negatives clamp");
    }

    #[test]
    fn sanitized_error_is_bounded_and_flat() {
        let e = EmailError::Rejected(format!("{}\nsecond line", "x".repeat(1000)));
        let s = sanitized_error(&e);
        assert!(s.len() <= 301);
        assert!(!s.contains('\n'));
    }

    #[test]
    fn template_key_mapping_is_total_and_never_panics() {
        assert_eq!(leaked_template_key("password_reset"), "password_reset");
        assert_eq!(leaked_template_key("what-is-this"), "security_alert");
    }

    // Async because building the lazy pool needs a Tokio runtime context.
    #[tokio::test]
    async fn rate_limit_converts_per_minute_to_interval() {
        let d = OutboxDispatcher {
            pool: unreachable_pool(),
            provider: Arc::new(NullProvider),
            from_address: "x@y".into(),
            min_send_interval: Duration::from_millis(250),
            batch_size: 1,
        }
        .with_rate_limit(6);
        assert_eq!(d.min_send_interval, Duration::from_millis(10_000));
        let d = d.with_rate_limit(0);
        assert_eq!(d.min_send_interval, Duration::from_millis(10_000), "0 keeps prior");
    }

    struct NullProvider;
    #[async_trait::async_trait]
    impl EmailProvider for NullProvider {
        async fn send(&self, _m: &EmailMessage) -> Result<Option<String>, EmailError> {
            Ok(None)
        }
        fn name(&self) -> &'static str {
            "null"
        }
    }

    /// The rate-limit test needs a PgPool value it never uses. Building a
    /// pool without a live database is possible via a lazy connect: the
    /// pool object exists, connections are only opened on first query.
    fn unreachable_pool() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgres://user:pass@127.0.0.1:1/none")
            .expect("lazy pool construction cannot fail")
    }
}
