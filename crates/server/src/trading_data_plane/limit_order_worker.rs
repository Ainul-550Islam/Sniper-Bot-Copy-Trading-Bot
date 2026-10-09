//! Durable limit-order worker (remediation tree, Part 2).
//!
//! The worker turns ACTIVE rows in `limit_orders` into trigger hand-offs. It
//! replaces the in-memory scheduler for server runtime. Guarantees:
//!
//! * **lease-based claim**: a row is claimed with
//!   `UPDATE … WHERE id IN (SELECT … FOR UPDATE SKIP LOCKED)`, so two replicas
//!   can never hold the same order at the same time;
//! * **fail-closed evaluation**: the decision is the module's own pure
//!   `evaluate()`. A missing or non-finite mark never fires an order;
//! * **hand-off before state change**: the order is marked `triggered` only
//!   AFTER `TriggerSink::hand_off` succeeds. A crash in between is
//!   at-least-once, so the sink MUST dedupe on the order id (documented below);
//! * **dead-letter, not silent loss**: a failed hand-off increments `attempts`,
//!   records `last_error`, releases the lease, and after
//!   [`MAX_HANDOFF_ATTEMPTS`] failures the row is no longer claimed. It stays
//!   visible through the API with its error;
//! * **attempts count failures only**: a healthy order that is simply waiting
//!   for its price is never dead-lettered, however many ticks it takes.
//!
//! The worker never sets an order to `expired` on a lease it does not hold,
//! and never fires one it does not hold, so a stale replica cannot act on a
//! row another replica now owns.

use std::time::Duration as StdDuration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use tokio::sync::watch;
use uuid::Uuid;

use module_sniper::limit_orders::{evaluate, LimitOrder, OrderSide, OrderStatus, TriggerKind};

/// Failed hand-offs before a row is parked as dead-letter.
pub const MAX_HANDOFF_ATTEMPTS: i32 = 5;
/// Rows claimed per tick.
pub const CLAIM_BATCH: i64 = 100;
/// Longest error text persisted, in characters.
pub const MAX_ERROR_CHARS: usize = 512;

/// Source of truth for a mint's current price (SOL per whole token).
/// `None` means "no trustworthy mark right now": the order keeps waiting.
#[async_trait]
pub trait MarkSource: Send + Sync {
    async fn mark_sol(&self, mint: &str) -> Option<f64>;
}

/// Downstream execution handoff.
///
/// **Idempotency contract:** the implementation MUST treat `order.id` as an
/// idempotency key. The worker may call `hand_off` more than once for the same
/// order after a crash or lease loss; a second call with the same id must be a
/// no-op that returns `Ok`.
#[async_trait]
pub trait TriggerSink: Send + Sync {
    async fn hand_off(&self, order: &ClaimedOrder, fired_at_sol: f64) -> Result<(), String>;
}

/// One row claimed under a lease.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimedOrder {
    pub id: String,
    pub organization_id: Uuid,
    pub mint: String,
    pub side: OrderSide,
    pub trigger: TriggerKind,
    pub price_sol: f64,
    pub amount: f64,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub attempts: i32,
}

impl ClaimedOrder {
    /// The pure order model the decision logic evaluates.
    pub fn as_model(&self) -> LimitOrder {
        LimitOrder {
            id: self.id.clone(),
            organization_id: self.organization_id.to_string(),
            mint: self.mint.clone(),
            side: self.side,
            trigger: self.trigger,
            price_sol: self.price_sol,
            amount: self.amount,
            status: OrderStatus::Active,
            created_at: self.created_at,
            expires_at: self.expires_at,
        }
    }
}

/// What to do with one claimed order at `now`, given the mark.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Price not met, or no trustworthy mark: release the lease, keep waiting.
    Wait,
    /// Past its expiry: mark expired. Expiry wins over a met price.
    Expire,
    /// Trigger met: hand off, then mark triggered.
    Fire { mark_sol: f64 },
}

/// Pure decision. Expiry is checked first, so an expired order can never fire.
pub fn decide(order: &LimitOrder, now: DateTime<Utc>, mark: Option<f64>) -> Decision {
    if let Some(expires_at) = order.expires_at {
        if now >= expires_at {
            return Decision::Expire;
        }
    }
    match mark {
        Some(m) if evaluate(order, m) => Decision::Fire { mark_sol: m },
        _ => Decision::Wait,
    }
}

/// Truncate an error message to the persisted length, on a char boundary.
pub fn clamp_error(message: &str) -> String {
    message.chars().take(MAX_ERROR_CHARS).collect()
}

/// Map the schema's side string to the model enum.
pub fn parse_side_column(raw: &str) -> Option<OrderSide> {
    match raw {
        "buy" => Some(OrderSide::Buy),
        "sell" => Some(OrderSide::Sell),
        _ => None,
    }
}

/// Map the schema's trigger string to the model enum.
pub fn parse_trigger_column(raw: &str) -> Option<TriggerKind> {
    match raw {
        "at_or_below" => Some(TriggerKind::AtOrBelow),
        "at_or_above" => Some(TriggerKind::AtOrAbove),
        _ => None,
    }
}

/// Worker tuning. `lease_secs` must comfortably exceed one hand-off call.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub worker_id: String,
    pub lease_secs: f64,
    pub tick: StdDuration,
}

/// Counts for one tick, for logs and metrics.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TickReport {
    pub claimed: usize,
    pub fired: usize,
    pub expired: usize,
    pub waiting: usize,
    pub failed: usize,
}

/// Claim up to [`CLAIM_BATCH`] due rows under a lease.
pub async fn claim_due(
    pool: &PgPool,
    worker_id: &str,
    lease_secs: f64,
) -> Result<Vec<ClaimedOrder>, sqlx::Error> {
    let rows = sqlx::query(
        "UPDATE limit_orders \
         SET lease_owner = $1, \
             lease_expires_at = now() + make_interval(secs => $2::float8) \
         WHERE id IN ( \
             SELECT id FROM limit_orders \
             WHERE status = 'active' \
               AND (lease_expires_at IS NULL OR lease_expires_at < now()) \
               AND attempts < $3 \
             ORDER BY created_at, id \
             LIMIT $4 \
             FOR UPDATE SKIP LOCKED) \
         RETURNING id, organization_id, mint, side, trigger_kind, \
                   price_sol::float8 AS price_sol, amount::float8 AS amount, \
                   created_at, expires_at, attempts",
    )
    .bind(worker_id)
    .bind(lease_secs)
    .bind(MAX_HANDOFF_ATTEMPTS)
    .bind(CLAIM_BATCH)
    .fetch_all(pool)
    .await?;

    let mut claimed = Vec::with_capacity(rows.len());
    for row in rows {
        let side_raw: String = row.get("side");
        let trigger_raw: String = row.get("trigger_kind");
        let (Some(side), Some(trigger)) = (
            parse_side_column(&side_raw),
            parse_trigger_column(&trigger_raw),
        ) else {
            // Schema CHECK makes this unreachable; if it ever happens, skip the
            // row and let its lease expire rather than guess a side or trigger.
            tracing::error!(order = %row.get::<String, _>("id"), "unknown side/trigger in claimed row");
            continue;
        };
        claimed.push(ClaimedOrder {
            id: row.get("id"),
            organization_id: row.get("organization_id"),
            mint: row.get("mint"),
            side,
            trigger,
            price_sol: row.get("price_sol"),
            amount: row.get("amount"),
            created_at: row.get("created_at"),
            expires_at: row.get("expires_at"),
            attempts: row.get("attempts"),
        });
    }
    Ok(claimed)
}

/// Release a lease we hold without changing the order (price not met).
async fn release_lease(
    pool: &PgPool,
    order: &ClaimedOrder,
    worker_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE limit_orders SET lease_owner = NULL, lease_expires_at = NULL \
         WHERE id = $1 AND organization_id = $2 AND lease_owner = $3",
    )
    .bind(&order.id)
    .bind(order.organization_id)
    .bind(worker_id)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Mark an order expired under the lease we hold.
async fn mark_expired(
    pool: &PgPool,
    order: &ClaimedOrder,
    worker_id: &str,
) -> Result<u64, sqlx::Error> {
    sqlx::query(
        "UPDATE limit_orders \
         SET status = 'expired', resolved_at = now(), lease_owner = NULL, lease_expires_at = NULL \
         WHERE id = $1 AND organization_id = $2 AND lease_owner = $3 AND status = 'active'",
    )
    .bind(&order.id)
    .bind(order.organization_id)
    .bind(worker_id)
    .execute(pool)
    .await
    .map(|r| r.rows_affected())
}

/// Mark an order triggered under the lease we hold, after a successful hand-off.
async fn mark_triggered(
    pool: &PgPool,
    order: &ClaimedOrder,
    worker_id: &str,
) -> Result<u64, sqlx::Error> {
    sqlx::query(
        "UPDATE limit_orders \
         SET status = 'triggered', resolved_at = now(), executed_at = now(), \
             lease_owner = NULL, lease_expires_at = NULL \
         WHERE id = $1 AND organization_id = $2 AND lease_owner = $3 AND status = 'active'",
    )
    .bind(&order.id)
    .bind(order.organization_id)
    .bind(worker_id)
    .execute(pool)
    .await
    .map(|r| r.rows_affected())
}

/// Record a failed hand-off and release the lease.
async fn record_handoff_failure(
    pool: &PgPool,
    order: &ClaimedOrder,
    worker_id: &str,
    reason: &str,
) -> Result<u64, sqlx::Error> {
    sqlx::query(
        "UPDATE limit_orders \
         SET attempts = attempts + 1, last_error = $4, lease_owner = NULL, lease_expires_at = NULL \
         WHERE id = $1 AND organization_id = $2 AND lease_owner = $3 AND status = 'active'",
    )
    .bind(&order.id)
    .bind(order.organization_id)
    .bind(worker_id)
    .bind(clamp_error(reason))
    .execute(pool)
    .await
    .map(|r| r.rows_affected())
}

/// One full pass: claim, decide, act. Per-order errors are counted and do not
/// abort the pass, so one bad row cannot starve the rest of the batch.
pub async fn tick_once(
    pool: &PgPool,
    cfg: &WorkerConfig,
    now: DateTime<Utc>,
    marks: &dyn MarkSource,
    sink: &dyn TriggerSink,
) -> Result<TickReport, sqlx::Error> {
    let claimed = claim_due(pool, &cfg.worker_id, cfg.lease_secs).await?;
    let mut report = TickReport {
        claimed: claimed.len(),
        ..TickReport::default()
    };

    for order in &claimed {
        let model = order.as_model();
        let mark = marks.mark_sol(&order.mint).await;
        match decide(&model, now, mark) {
            Decision::Wait => {
                report.waiting += 1;
                if let Err(error) = release_lease(pool, order, &cfg.worker_id).await {
                    tracing::warn!(order = %order.id, error = %error, "lease release failed; will expire");
                }
            }
            Decision::Expire => match mark_expired(pool, order, &cfg.worker_id).await {
                Ok(_) => report.expired += 1,
                Err(error) => {
                    report.failed += 1;
                    tracing::error!(order = %order.id, error = %error, "expiring order failed");
                }
            },
            Decision::Fire { mark_sol } => {
                if let Err(reason) = sink.hand_off(order, mark_sol).await {
                    report.failed += 1;
                    if let Err(error) =
                        record_handoff_failure(pool, order, &cfg.worker_id, &reason).await
                    {
                        tracing::error!(order = %order.id, error = %error, "recording hand-off failure failed");
                    }
                    continue;
                }
                match mark_triggered(pool, order, &cfg.worker_id).await {
                    Ok(1) => report.fired += 1,
                    Ok(_) => {
                        // Lease lost after hand-off. The sink deduplicates on
                        // order id, so this is safe; it is logged for review.
                        tracing::warn!(order = %order.id, "lease lost after hand-off; sink dedupes");
                        report.fired += 1;
                    }
                    Err(error) => {
                        report.failed += 1;
                        tracing::error!(order = %order.id, error = %error, "marking triggered failed; sink dedupes on retry");
                    }
                }
            }
        }
    }
    Ok(report)
}

/// Run until `stop` flips to `true`. Storage errors back off and retry; the
/// loop never exits on a transient failure.
pub async fn run(
    pool: PgPool,
    cfg: WorkerConfig,
    marks: std::sync::Arc<dyn MarkSource>,
    sink: std::sync::Arc<dyn TriggerSink>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        if *stop.borrow() {
            break;
        }
        let pause = match tick_once(&pool, &cfg, Utc::now(), marks.as_ref(), sink.as_ref()).await {
            Ok(_) => cfg.tick,
            Err(error) => {
                tracing::error!(error = %error, "limit-order tick failed; backing off");
                cfg.tick.max(StdDuration::from_secs(5))
            }
        };
        tokio::select! {
            _ = tokio::time::sleep(pause) => {}
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn model(trigger: TriggerKind, price: f64, expires_at: Option<DateTime<Utc>>) -> LimitOrder {
        LimitOrder {
            id: "lo_t".into(),
            organization_id: Uuid::nil().to_string(),
            mint: "So11111111111111111111111111111111111111112".into(),
            side: OrderSide::Buy,
            trigger,
            price_sol: price,
            amount: 1.0,
            status: OrderStatus::Active,
            created_at: DateTime::from_timestamp(1_000_000, 0).unwrap(),
            expires_at,
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_000_100, 0).unwrap()
    }

    #[test]
    fn no_mark_means_wait_never_fire() {
        let o = model(TriggerKind::AtOrBelow, 1.0, None);
        assert_eq!(decide(&o, now(), None), Decision::Wait);
        // Non-finite and non-positive marks are not trustworthy: wait.
        assert_eq!(decide(&o, now(), Some(f64::NAN)), Decision::Wait);
        assert_eq!(decide(&o, now(), Some(0.0)), Decision::Wait);
    }

    #[test]
    fn met_price_fires_with_the_mark() {
        let o = model(TriggerKind::AtOrBelow, 1.0, None);
        assert_eq!(
            decide(&o, now(), Some(0.9)),
            Decision::Fire { mark_sol: 0.9 }
        );
        let above = model(TriggerKind::AtOrAbove, 1.0, None);
        assert_eq!(
            decide(&above, now(), Some(1.5)),
            Decision::Fire { mark_sol: 1.5 }
        );
        assert_eq!(decide(&above, now(), Some(0.5)), Decision::Wait);
    }

    #[test]
    fn expiry_beats_a_met_price() {
        let expired = model(
            TriggerKind::AtOrBelow,
            1.0,
            Some(now() - Duration::seconds(1)),
        );
        assert_eq!(decide(&expired, now(), Some(0.1)), Decision::Expire);
        // Exactly at the deadline counts as expired (>=), matching the pure model.
        let at_deadline = model(TriggerKind::AtOrBelow, 1.0, Some(now()));
        assert_eq!(decide(&at_deadline, now(), Some(0.1)), Decision::Expire);
    }

    #[test]
    fn unexpired_order_with_future_deadline_still_evaluates() {
        let later = model(
            TriggerKind::AtOrBelow,
            1.0,
            Some(now() + Duration::hours(1)),
        );
        assert_eq!(
            decide(&later, now(), Some(0.5)),
            Decision::Fire { mark_sol: 0.5 }
        );
    }

    #[test]
    fn column_parsers_reject_unknown_values() {
        assert_eq!(parse_side_column("buy"), Some(OrderSide::Buy));
        assert_eq!(parse_side_column("BUY"), None);
        assert_eq!(
            parse_trigger_column("at_or_above"),
            Some(TriggerKind::AtOrAbove)
        );
        assert_eq!(parse_trigger_column("sometimes"), None);
    }

    #[test]
    fn error_text_is_bounded_on_char_boundaries() {
        let long = "é".repeat(MAX_ERROR_CHARS + 50);
        let clamped = clamp_error(&long);
        assert_eq!(clamped.chars().count(), MAX_ERROR_CHARS);
        assert_eq!(clamp_error("short"), "short");
    }

    #[test]
    fn dead_letter_threshold_is_positive() {
        assert!(MAX_HANDOFF_ATTEMPTS > 0);
    }
}
