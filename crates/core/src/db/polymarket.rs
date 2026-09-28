//! Durable Polymarket trading repository (migration 0014, TASK 4).
//!
//! Backs Module 3's signal journal, CLOB order lifecycle, fills and
//! reconciliation findings with PostgreSQL. The generic OMS tables (0002)
//! remain the ONE authoritative order record + idempotency boundary; these
//! tables hold the venue-specific detail (venue order id, matched size,
//! venue status string, fill events) the OMS deliberately does not model.
//!
//! Same conventions as `copy.rs`: runtime-checked queries, upserts / guarded
//! inserts, every call through [`Database::timed`]. The record types are
//! plain data so `module-polymarket` can use them without depending on
//! `sqlx`.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::db::{Database, TimedDbError};

type RepoResult<T> = Result<T, TimedDbError>;

/// One strategy decision the pipeline finished with (accepted or rejected).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolySignalRecord {
    /// Deterministic signal id (`psig_` + digest of the intent identity).
    pub signal_id: String,
    pub condition_id: String,
    pub token_id: String,
    pub outcome: String,
    /// `buy` | `sell`.
    pub side: String,
    pub strategy: String,
    pub limit_price: f64,
    pub size_tokens: f64,
    pub stake_usd: f64,
    /// `paper` | `simulate` | `live`.
    pub mode: String,
    /// Last pipeline stage reached.
    pub stage: String,
    pub reject_reason: Option<String>,
    pub detail: String,
    /// OMS order id once the idempotency gate was passed.
    pub order_id: Option<String>,
    /// Venue order id once the order was signed.
    pub venue_order_id: Option<String>,
    pub position_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// CLOB-level lifecycle of one venue order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolyOrderRecord {
    /// Venue order id (`0x` + EIP-712 struct hash) or `paper:…`.
    pub venue_order_id: String,
    /// The OMS `orders.id` this venue order belongs to.
    pub order_id: String,
    pub signal_id: String,
    pub condition_id: String,
    pub token_id: String,
    pub outcome: String,
    /// `buy` | `sell`.
    pub side: String,
    pub order_type: String,
    pub limit_price: f64,
    pub size_tokens: f64,
    pub size_matched: f64,
    /// `paper` | `simulate` | `live`.
    pub mode: String,
    /// Local lifecycle state (`submitted`, `resting`, `partially_filled`,
    /// `filled`, `cancelled`, `expired`, `unknown`, `failed`).
    pub state: String,
    /// Raw venue status string last observed.
    pub venue_status: String,
    /// Unix seconds; 0 = GTC.
    pub expiration: i64,
    pub position_id: Option<String>,
    pub replica_id: String,
    pub submitted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

/// One fill event booked against a venue order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolyFillRecord {
    /// Venue trade id or a deterministic digest (see migration 0014).
    pub fill_id: String,
    pub venue_order_id: String,
    pub order_id: String,
    pub token_id: String,
    /// `buy` | `sell`.
    pub side: String,
    pub price: f64,
    pub size_tokens: f64,
    pub quote_usd: f64,
    /// `poll` | `user_ws` | `paper` | `recon`.
    pub source: String,
    pub position_id: Option<String>,
    pub ts: DateTime<Utc>,
}

/// One local-vs-venue reconciliation finding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolyReconFindingRecord {
    #[serde(default)]
    pub id: i64,
    pub kind: String,
    pub venue_order_id: Option<String>,
    pub order_id: Option<String>,
    pub token_id: Option<String>,
    pub detail: String,
    pub action: String,
    pub replica_id: String,
    pub ts: DateTime<Utc>,
}

pub struct PolyRepo {
    db: Arc<Database>,
}

impl PolyRepo {
    pub fn new(db: Arc<Database>) -> Self {
        PolyRepo { db }
    }

    // ------------------------------------------------------------ signals --

    /// Upsert the final outcome of one signal. Idempotent: the same snapshot
    /// is a no-op; a later outcome overwrites stage/reason/detail/ids while
    /// `created_at` keeps the first-seen time.
    pub async fn record_signal(&self, rec: &PolySignalRecord) -> RepoResult<()> {
        self.db
            .timed(
                "poly_signal_record",
                sqlx::query(
                    r#"INSERT INTO poly_signals
                           (signal_id, condition_id, token_id, outcome, side, strategy,
                            limit_price, size_tokens, stake_usd, mode, stage, reject_reason,
                            detail, order_id, venue_order_id, position_id, created_at, updated_at)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                               $13, $14, $15, $16, $17, $18)
                       ON CONFLICT (signal_id) DO UPDATE SET
                           stage = EXCLUDED.stage,
                           reject_reason = EXCLUDED.reject_reason,
                           detail = EXCLUDED.detail,
                           order_id = COALESCE(EXCLUDED.order_id, poly_signals.order_id),
                           venue_order_id = COALESCE(EXCLUDED.venue_order_id, poly_signals.venue_order_id),
                           position_id = COALESCE(EXCLUDED.position_id, poly_signals.position_id),
                           updated_at = EXCLUDED.updated_at"#,
                )
                .bind(&rec.signal_id)
                .bind(&rec.condition_id)
                .bind(&rec.token_id)
                .bind(&rec.outcome)
                .bind(&rec.side)
                .bind(&rec.strategy)
                .bind(rec.limit_price)
                .bind(rec.size_tokens)
                .bind(rec.stake_usd)
                .bind(&rec.mode)
                .bind(&rec.stage)
                .bind(&rec.reject_reason)
                .bind(&rec.detail)
                .bind(&rec.order_id)
                .bind(&rec.venue_order_id)
                .bind(&rec.position_id)
                .bind(rec.created_at)
                .bind(rec.updated_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    pub async fn get_signal(&self, signal_id: &str) -> RepoResult<Option<PolySignalRecord>> {
        let row = self
            .db
            .timed(
                "poly_signal_get",
                sqlx::query("SELECT * FROM poly_signals WHERE signal_id = $1")
                    .bind(signal_id)
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.as_ref().map(signal_from_row))
    }

    /// Signals updated since `since`, oldest first (restart recovery / audit).
    pub async fn signals_since(
        &self,
        since: DateTime<Utc>,
        limit: i64,
    ) -> RepoResult<Vec<PolySignalRecord>> {
        let rows = self
            .db
            .timed(
                "poly_signals_since",
                sqlx::query(
                    r#"SELECT * FROM poly_signals
                        WHERE updated_at >= $1 ORDER BY updated_at ASC, signal_id ASC LIMIT $2"#,
                )
                .bind(since)
                .bind(limit.clamp(1, 10_000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(signal_from_row).collect())
    }

    // ------------------------------------------------------------- orders --

    /// Upsert one venue order's lifecycle snapshot. `submitted_at` keeps the
    /// first time the order reached the venue; `size_matched` never goes
    /// backwards (a stale poll cannot un-fill an order).
    pub async fn upsert_order(&self, rec: &PolyOrderRecord) -> RepoResult<()> {
        self.db
            .timed(
                "poly_order_upsert",
                sqlx::query(
                    r#"INSERT INTO poly_orders
                           (venue_order_id, order_id, signal_id, condition_id, token_id, outcome,
                            side, order_type, limit_price, size_tokens, size_matched, mode, state,
                            venue_status, expiration, position_id, replica_id, submitted_at,
                            updated_at, closed_at)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13,
                               $14, $15, $16, $17, $18, $19, $20)
                       ON CONFLICT (venue_order_id) DO UPDATE SET
                           order_id = EXCLUDED.order_id,
                           size_matched = GREATEST(poly_orders.size_matched, EXCLUDED.size_matched),
                           state = EXCLUDED.state,
                           venue_status = EXCLUDED.venue_status,
                           position_id = COALESCE(EXCLUDED.position_id, poly_orders.position_id),
                           replica_id = EXCLUDED.replica_id,
                           updated_at = EXCLUDED.updated_at,
                           closed_at = COALESCE(EXCLUDED.closed_at, poly_orders.closed_at)"#,
                )
                .bind(&rec.venue_order_id)
                .bind(&rec.order_id)
                .bind(&rec.signal_id)
                .bind(&rec.condition_id)
                .bind(&rec.token_id)
                .bind(&rec.outcome)
                .bind(&rec.side)
                .bind(&rec.order_type)
                .bind(rec.limit_price)
                .bind(rec.size_tokens)
                .bind(rec.size_matched)
                .bind(&rec.mode)
                .bind(&rec.state)
                .bind(&rec.venue_status)
                .bind(rec.expiration)
                .bind(&rec.position_id)
                .bind(&rec.replica_id)
                .bind(rec.submitted_at)
                .bind(rec.updated_at)
                .bind(rec.closed_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    pub async fn get_order(&self, venue_order_id: &str) -> RepoResult<Option<PolyOrderRecord>> {
        let row = self
            .db
            .timed(
                "poly_order_get",
                sqlx::query("SELECT * FROM poly_orders WHERE venue_order_id = $1")
                    .bind(venue_order_id)
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.as_ref().map(order_from_row))
    }

    /// Every venue order that has not reached a terminal state (restart
    /// recovery re-adopts these and asks the venue for the truth).
    pub async fn open_orders(&self) -> RepoResult<Vec<PolyOrderRecord>> {
        let rows = self
            .db
            .timed(
                "poly_orders_open",
                sqlx::query(
                    r#"SELECT * FROM poly_orders
                        WHERE closed_at IS NULL ORDER BY submitted_at ASC, venue_order_id ASC"#,
                )
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(order_from_row).collect())
    }

    // -------------------------------------------------------------- fills --

    /// Guarded insert of one fill. Returns `true` when the row was new,
    /// `false` when the fill id was already booked (replayed event).
    pub async fn record_fill(&self, rec: &PolyFillRecord) -> RepoResult<bool> {
        let res = self
            .db
            .timed(
                "poly_fill_record",
                sqlx::query(
                    r#"INSERT INTO poly_fills
                           (fill_id, venue_order_id, order_id, token_id, side, price,
                            size_tokens, quote_usd, source, position_id, ts)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                       ON CONFLICT (fill_id) DO NOTHING"#,
                )
                .bind(&rec.fill_id)
                .bind(&rec.venue_order_id)
                .bind(&rec.order_id)
                .bind(&rec.token_id)
                .bind(&rec.side)
                .bind(rec.price)
                .bind(rec.size_tokens)
                .bind(rec.quote_usd)
                .bind(&rec.source)
                .bind(&rec.position_id)
                .bind(rec.ts)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Fills booked against one venue order, oldest first.
    pub async fn fills_for(&self, venue_order_id: &str) -> RepoResult<Vec<PolyFillRecord>> {
        let rows = self
            .db
            .timed(
                "poly_fills_for",
                sqlx::query(
                    r#"SELECT * FROM poly_fills WHERE venue_order_id = $1
                        ORDER BY ts ASC, fill_id ASC"#,
                )
                .bind(venue_order_id)
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(fill_from_row).collect())
    }

    // ----------------------------------------------------- recon findings --

    pub async fn append_finding(&self, rec: &PolyReconFindingRecord) -> RepoResult<()> {
        self.db
            .timed(
                "poly_recon_finding",
                sqlx::query(
                    r#"INSERT INTO poly_recon_findings
                           (kind, venue_order_id, order_id, token_id, detail, action, replica_id, ts)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"#,
                )
                .bind(&rec.kind)
                .bind(&rec.venue_order_id)
                .bind(&rec.order_id)
                .bind(&rec.token_id)
                .bind(&rec.detail)
                .bind(&rec.action)
                .bind(&rec.replica_id)
                .bind(rec.ts)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Most recent findings, newest first.
    pub async fn recent_findings(&self, limit: i64) -> RepoResult<Vec<PolyReconFindingRecord>> {
        let rows = self
            .db
            .timed(
                "poly_recon_recent",
                sqlx::query(
                    r#"SELECT * FROM poly_recon_findings ORDER BY ts DESC, id DESC LIMIT $1"#,
                )
                .bind(limit.clamp(1, 1000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(finding_from_row).collect())
    }
}

fn signal_from_row(r: &PgRow) -> PolySignalRecord {
    PolySignalRecord {
        signal_id: r.try_get("signal_id").unwrap_or_default(),
        condition_id: r.try_get("condition_id").unwrap_or_default(),
        token_id: r.try_get("token_id").unwrap_or_default(),
        outcome: r.try_get("outcome").unwrap_or_default(),
        side: r.try_get("side").unwrap_or_default(),
        strategy: r.try_get("strategy").unwrap_or_default(),
        limit_price: r.try_get("limit_price").unwrap_or_default(),
        size_tokens: r.try_get("size_tokens").unwrap_or_default(),
        stake_usd: r.try_get("stake_usd").unwrap_or_default(),
        mode: r.try_get("mode").unwrap_or_default(),
        stage: r.try_get("stage").unwrap_or_default(),
        reject_reason: r.try_get("reject_reason").unwrap_or_default(),
        detail: r.try_get("detail").unwrap_or_default(),
        order_id: r.try_get("order_id").unwrap_or_default(),
        venue_order_id: r.try_get("venue_order_id").unwrap_or_default(),
        position_id: r.try_get("position_id").unwrap_or_default(),
        created_at: r.try_get("created_at").unwrap_or_else(|_| Utc::now()),
        updated_at: r.try_get("updated_at").unwrap_or_else(|_| Utc::now()),
    }
}

fn order_from_row(r: &PgRow) -> PolyOrderRecord {
    PolyOrderRecord {
        venue_order_id: r.try_get("venue_order_id").unwrap_or_default(),
        order_id: r.try_get("order_id").unwrap_or_default(),
        signal_id: r.try_get("signal_id").unwrap_or_default(),
        condition_id: r.try_get("condition_id").unwrap_or_default(),
        token_id: r.try_get("token_id").unwrap_or_default(),
        outcome: r.try_get("outcome").unwrap_or_default(),
        side: r.try_get("side").unwrap_or_default(),
        order_type: r.try_get("order_type").unwrap_or_default(),
        limit_price: r.try_get("limit_price").unwrap_or_default(),
        size_tokens: r.try_get("size_tokens").unwrap_or_default(),
        size_matched: r.try_get("size_matched").unwrap_or_default(),
        mode: r.try_get("mode").unwrap_or_default(),
        state: r.try_get("state").unwrap_or_default(),
        venue_status: r.try_get("venue_status").unwrap_or_default(),
        expiration: r.try_get("expiration").unwrap_or_default(),
        position_id: r.try_get("position_id").unwrap_or_default(),
        replica_id: r.try_get("replica_id").unwrap_or_default(),
        submitted_at: r.try_get("submitted_at").unwrap_or_else(|_| Utc::now()),
        updated_at: r.try_get("updated_at").unwrap_or_else(|_| Utc::now()),
        closed_at: r.try_get("closed_at").unwrap_or_default(),
    }
}

fn fill_from_row(r: &PgRow) -> PolyFillRecord {
    PolyFillRecord {
        fill_id: r.try_get("fill_id").unwrap_or_default(),
        venue_order_id: r.try_get("venue_order_id").unwrap_or_default(),
        order_id: r.try_get("order_id").unwrap_or_default(),
        token_id: r.try_get("token_id").unwrap_or_default(),
        side: r.try_get("side").unwrap_or_default(),
        price: r.try_get("price").unwrap_or_default(),
        size_tokens: r.try_get("size_tokens").unwrap_or_default(),
        quote_usd: r.try_get("quote_usd").unwrap_or_default(),
        source: r.try_get("source").unwrap_or_default(),
        position_id: r.try_get("position_id").unwrap_or_default(),
        ts: r.try_get("ts").unwrap_or_else(|_| Utc::now()),
    }
}

fn finding_from_row(r: &PgRow) -> PolyReconFindingRecord {
    PolyReconFindingRecord {
        id: r.try_get("id").unwrap_or_default(),
        kind: r.try_get("kind").unwrap_or_default(),
        venue_order_id: r.try_get("venue_order_id").unwrap_or_default(),
        order_id: r.try_get("order_id").unwrap_or_default(),
        token_id: r.try_get("token_id").unwrap_or_default(),
        detail: r.try_get("detail").unwrap_or_default(),
        action: r.try_get("action").unwrap_or_default(),
        replica_id: r.try_get("replica_id").unwrap_or_default(),
        ts: r.try_get("ts").unwrap_or_else(|_| Utc::now()),
    }
}
