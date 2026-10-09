//! Webhook delivery + retry flow (GAP-MAP v2 P1).
//!
//! The audit's VERIFY item found that `webhooks.rs` attempted one delivery
//! and nothing ever retried a failure; Part 2 added
//! `saas/webhook_delivery.rs` (dispatcher, SKIP-LOCKED claim, exponential
//! backoff, dead letter after MAX_ATTEMPTS) plus migration 0048. This file
//! pins the behaviour:
//!
//! 1. **Hermetic** — the backoff schedule and the delivery-level
//!    classification (bad URL, non-HTTPS URL) need no database.
//! 2. **Durable (`POSTGRES_URL` required, else NOT_RUN)** — the dispatcher
//!    claims only DUE failed rows, dead-letters rows that exhaust
//!    MAX_ATTEMPTS, never touches pending rows or rows at the attempt cap,
//!    and leaves future-dated retries alone.

use std::sync::Arc;

use uuid::Uuid;

use bot_core::config::DatabaseConfig;
use bot_core::db::Database;

use sniper_suite::saas::webhook_delivery::{deliver_once, retry_backoff, WebhookRetryDispatcher, MAX_ATTEMPTS};

// ---------------------------------------------------------------------
// Hermetic: backoff schedule + URL classification
// ---------------------------------------------------------------------

#[test]
fn backoff_schedule_is_exponential_capped_and_bounded() {
    // 60s * 2^n, capped at 30 minutes, for attempts 1..=MAX.
    let first = retry_backoff(1).num_seconds();
    assert!(first >= 30 && first <= 120, "first retry ~1 minute, got {first}");
    let second = retry_backoff(2).num_seconds();
    assert!(second > first, "backoff must grow");
    assert_eq!(retry_backoff(MAX_ATTEMPTS).num_seconds(), 1800, "cap = 30 min");
    // Degenerate inputs must not panic and stay within the cap.
    for attempt in [-5, 0, i32::MAX / 2, i32::MAX] {
        let d = retry_backoff(attempt).num_seconds();
        assert!(d >= 0 && d <= 1800, "attempt {attempt} → {d}s");
    }
}

#[tokio::test]
async fn malformed_and_non_https_urls_are_rejected_before_any_io() {
    for url in ["not a url", "http://insecure.example.com/hook", "ftp://example.com"] {
        let outcome = deliver_once(url, "whsec_test", "evt_1", b"{}").await;
        assert!(!outcome.succeeded, "{url} must never deliver");
        assert!(outcome.response_status.is_none());
        assert!(outcome.error.is_some(), "{url} must carry a reason");
    }
}

// ---------------------------------------------------------------------
// Durable dispatcher flow (POSTGRES_URL required)
// ---------------------------------------------------------------------

fn pg_url() -> Option<String> {
    std::env::var("POSTGRES_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

async fn setup_db() -> Option<Arc<Database>> {
    let url = pg_url()?;
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Database::connect(&cfg, &url).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(Arc::new(db))
}

/// Seed org → endpoint → delivery row and return (org, endpoint, delivery).
async fn seed_delivery(
    db: &Database,
    suffix: &str,
    status: &str,
    attempt_count: i32,
    next_retry_sql: &str,
) -> (Uuid, Uuid, Uuid) {
    let org = Uuid::new_v4();
    let endpoint = Uuid::new_v4();
    let delivery = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
        .bind(org)
        .bind(format!("wh-{suffix}-{}", &org.to_string()[..8]))
        .bind("Webhook Test Org")
        .execute(db.pool())
        .await
        .expect("seed org");
    sqlx::query(
        "INSERT INTO webhook_endpoints (id, organization_id, url, secret, status)
         VALUES ($1, $2, $3, $4, 'active')",
    )
    .bind(endpoint)
    .bind(org)
    .bind("https://nonexistent.invalid/webhook")
    .bind("whsec_test_secret")
    .execute(db.pool())
    .await
    .expect("seed endpoint");
    let insert = format!(
        "INSERT INTO webhook_deliveries
             (id, organization_id, endpoint_id, event_id, event_type, status,
              attempt_count, payload, next_retry_at)
         VALUES ($1, $2, $3, 'evt_{suffix}', 'test.event', $4, $5, '{{}}'::jsonb, {next_retry_sql})"
    );
    sqlx::query(&insert)
        .bind(delivery)
        .bind(org)
        .bind(endpoint)
        .bind(status)
        .bind(attempt_count)
        .execute(db.pool())
        .await
        .expect("seed delivery");
    (org, endpoint, delivery)
}

async fn delivery_state(db: &Database, id: Uuid) -> (String, i32) {
    let row = sqlx::query("SELECT status, attempt_count FROM webhook_deliveries WHERE id = $1")
        .bind(id)
        .fetch_one(db.pool())
        .await
        .expect("delivery row");
    use sqlx::Row;
    (row.get("status"), row.get("attempt_count"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatcher_dead_letters_rows_that_exhaust_max_attempts() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: webhook dead-letter — POSTGRES_URL missing");
        return;
    };
    // One attempt away from the cap, already due.
    let (_, _, delivery) = seed_delivery(
        &db,
        "dead",
        "failed",
        MAX_ATTEMPTS - 1,
        "now() - interval '1 second'",
    )
    .await;
    let dispatcher = WebhookRetryDispatcher::new(db.pool().clone());
    let processed = dispatcher.run_once().await.expect("run_once");
    assert_eq!(processed, 1, "the due row must be claimed");
    let (status, attempts) = delivery_state(&db, delivery).await;
    assert_eq!(status, "dead", "exhausted rows become the dead letter");
    assert_eq!(attempts, MAX_ATTEMPTS);
    // A second sweep finds nothing: dead rows are terminal.
    let processed = dispatcher.run_once().await.expect("run_once 2");
    assert_eq!(processed, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatcher_ignores_pending_future_and_exhausted_rows() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: webhook claim scope — POSTGRES_URL missing");
        return;
    };
    // Pending rows are the live path — the retry dispatcher must not
    // touch them.
    let (_, _, pending) = seed_delivery(&db, "pend", "pending", 1, "now()").await;
    // Future-dated retry: not due yet.
    let (_, _, future) = seed_delivery(
        &db,
        "future",
        "failed",
        2,
        "now() + interval '1 hour'",
    )
    .await;
    // Already at the attempt cap: the claim's WHERE clause excludes it
    // (it can only become dead through a claimed attempt).
    let (_, _, capped) = seed_delivery(
        &db,
        "capped",
        "failed",
        MAX_ATTEMPTS,
        "now() - interval '1 second'",
    )
    .await;

    let dispatcher = WebhookRetryDispatcher::new(db.pool().clone());
    let processed = dispatcher.run_once().await.expect("run_once");
    assert_eq!(processed, 0, "nothing in this seed is due");

    assert_eq!(delivery_state(&db, pending).await, ("pending".into(), 1));
    assert_eq!(delivery_state(&db, future).await, ("failed".into(), 2));
    assert_eq!(delivery_state(&db, capped).await, ("failed".into(), MAX_ATTEMPTS));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_retry_reschedules_with_backoff_until_exhausted() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: webhook reschedule — POSTGRES_URL missing");
        return;
    };
    // Early in the retry ladder: a failed attempt must schedule the next
    // one (not die, not succeed).
    let (_, _, delivery) = seed_delivery(
        &db,
        "retry",
        "failed",
        1,
        "now() - interval '1 second'",
    )
    .await;
    let dispatcher = WebhookRetryDispatcher::new(db.pool().clone());
    let processed = dispatcher.run_once().await.expect("run_once");
    assert_eq!(processed, 1);
    let (status, attempts) = delivery_state(&db, delivery).await;
    assert_eq!(status, "failed", "still retryable, not dead");
    assert_eq!(attempts, 2);
    let row = sqlx::query("SELECT next_retry_at FROM webhook_deliveries WHERE id = $1")
        .bind(delivery)
        .fetch_one(db.pool())
        .await
        .expect("row");
    use sqlx::Row;
    let next: Option<chrono::DateTime<chrono::Utc>> = row.get("next_retry_at");
    let next = next.expect("a retry must be scheduled");
    assert!(
        next > chrono::Utc::now(),
        "next attempt must be in the future (backoff), got {next}"
    );
}
