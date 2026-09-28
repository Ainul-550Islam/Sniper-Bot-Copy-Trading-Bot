//! Repository layer — every SQL statement in the codebase lives here.
//!
//! Conventions:
//! * Runtime-checked queries (`sqlx::query`), never `query!` macros: the
//!   build does not need a live database.
//! * Writes are upserts or guarded inserts so retries after ambiguous
//!   failures cannot duplicate state.
//! * Every call goes through [`Database::timed`] (timeout + metering).
//! * Money/quantities are `double precision` mirroring the app's f64 model;
//!   the DB never performs money arithmetic.

use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::db::{Database, TimedDbError};
use crate::models::{
    BotModule, ExecutionMode, Position, PositionSide, PositionStatus, Trade, TradeSource, Venue,
};
use crate::oms::{ExecutionRecord, Order, OrderStatus};

type RepoResult<T> = Result<T, TimedDbError>;

fn ts(row: &PgRow, col: &str) -> Option<DateTime<Utc>> {
    row.try_get::<DateTime<Utc>, _>(col).ok()
}

// ---------------------------------------------------------------------------
// Orders
// ---------------------------------------------------------------------------

pub struct OrderRepo {
    db: Arc<Database>,
}

impl OrderRepo {
    pub fn new(db: Arc<Database>) -> Self {
        OrderRepo { db }
    }

    fn map(row: &PgRow) -> Order {
        let module_s: String = row.try_get("module").unwrap_or_default();
        let mode_s: String = row.try_get("mode").unwrap_or_default();
        let status_s: String = row.try_get("status").unwrap_or_default();
        Order {
            id: row.try_get("id").unwrap_or_default(),
            idempotency_key: row.try_get("idempotency_key").unwrap_or_default(),
            module: BotModule::from_str(&module_s).unwrap_or(BotModule::Sniper),
            side: row.try_get("side").unwrap_or_default(),
            symbol: row.try_get("symbol").unwrap_or_default(),
            venue: row.try_get("venue").unwrap_or_default(),
            mode: ExecutionMode::from_str(&mode_s).unwrap_or(ExecutionMode::Paper),
            status: OrderStatus::parse(&status_s).unwrap_or(OrderStatus::Unknown),
            qty: row.try_get("qty").unwrap_or(0.0),
            price: row.try_get("price").ok(),
            external_id: row.try_get("external_id").ok(),
            signature: row.try_get("signature").ok(),
            error: row.try_get("error").ok(),
            meta: row.try_get("meta").unwrap_or(serde_json::json!({})),
            created_at: ts(row, "created_at").unwrap_or_else(Utc::now),
            updated_at: ts(row, "updated_at").unwrap_or_else(Utc::now),
            submitted_at: ts(row, "submitted_at"),
            finished_at: ts(row, "finished_at"),
        }
    }

    /// Insert unless the idempotency key already exists.
    /// `Ok(true)` = inserted, `Ok(false)` = duplicate key (caller should
    /// fetch + reuse the existing order).
    pub async fn insert_if_absent(&self, o: &Order) -> RepoResult<bool> {
        let res = self
            .db
            .timed(
                "order_insert",
                sqlx::query(
                    r#"INSERT INTO orders
                        (id, idempotency_key, module, side, symbol, venue, mode,
                         status, qty, price, external_id, signature, error, meta,
                         created_at, updated_at, submitted_at, finished_at)
                       VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)
                       ON CONFLICT (idempotency_key) DO NOTHING"#,
                )
                .bind(&o.id)
                .bind(&o.idempotency_key)
                .bind(o.module.as_str())
                .bind(&o.side)
                .bind(&o.symbol)
                .bind(&o.venue)
                .bind(o.mode.as_str())
                .bind(o.status.as_str())
                .bind(o.qty)
                .bind(o.price)
                .bind(&o.external_id)
                .bind(&o.signature)
                .bind(&o.error)
                .bind(&o.meta)
                .bind(o.created_at)
                .bind(o.updated_at)
                .bind(o.submitted_at)
                .bind(o.finished_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Full upsert used for mirror refresh + recovery writes.
    pub async fn upsert(&self, o: &Order) -> RepoResult<()> {
        self.db
            .timed(
                "order_upsert",
                sqlx::query(
                    r#"INSERT INTO orders
                        (id, idempotency_key, module, side, symbol, venue, mode,
                         status, qty, price, external_id, signature, error, meta,
                         created_at, updated_at, submitted_at, finished_at)
                       VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)
                       ON CONFLICT (id) DO UPDATE SET
                         status = EXCLUDED.status,
                         qty = EXCLUDED.qty,
                         price = EXCLUDED.price,
                         external_id = COALESCE(EXCLUDED.external_id, orders.external_id),
                         signature = COALESCE(EXCLUDED.signature, orders.signature),
                         error = EXCLUDED.error,
                         meta = EXCLUDED.meta,
                         updated_at = EXCLUDED.updated_at,
                         submitted_at = COALESCE(EXCLUDED.submitted_at, orders.submitted_at),
                         finished_at = EXCLUDED.finished_at"#,
                )
                .bind(&o.id)
                .bind(&o.idempotency_key)
                .bind(o.module.as_str())
                .bind(&o.side)
                .bind(&o.symbol)
                .bind(&o.venue)
                .bind(o.mode.as_str())
                .bind(o.status.as_str())
                .bind(o.qty)
                .bind(o.price)
                .bind(&o.external_id)
                .bind(&o.signature)
                .bind(&o.error)
                .bind(&o.meta)
                .bind(o.created_at)
                .bind(o.updated_at)
                .bind(o.submitted_at)
                .bind(o.finished_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Status transition + history row in one statement batch.
    pub async fn set_status(
        &self,
        o: &Order,
        from: OrderStatus,
        reason: Option<&str>,
    ) -> RepoResult<()> {
        let mut tx = self.db.pool().begin().await.map_err(TimedDbError::Error)?;
        sqlx::query(
            r#"UPDATE orders SET status = $2, updated_at = $3,
                     submitted_at = COALESCE(submitted_at, $4),
                     finished_at = $5, error = COALESCE($6, error)
               WHERE id = $1"#,
        )
        .bind(&o.id)
        .bind(o.status.as_str())
        .bind(o.updated_at)
        .bind(o.submitted_at)
        .bind(o.finished_at)
        .bind(if o.status == OrderStatus::Failed {
            o.error.clone()
        } else {
            None
        })
        .execute(&mut *tx)
        .await
        .map_err(TimedDbError::Error)?;
        sqlx::query(
            r#"INSERT INTO order_status_history (order_id, from_status, to_status, reason)
               VALUES ($1, $2, $3, $4)"#,
        )
        .bind(&o.id)
        .bind(from.as_str())
        .bind(o.status.as_str())
        .bind(reason)
        .execute(&mut *tx)
        .await
        .map_err(TimedDbError::Error)?;
        tx.commit().await.map_err(TimedDbError::Error)?;
        Ok(())
    }

    pub async fn update_external(&self, o: &Order) -> RepoResult<()> {
        self.db
            .timed(
                "order_external",
                sqlx::query(
                    r#"UPDATE orders SET external_id = COALESCE($2, external_id),
                              signature = COALESCE($3, signature), updated_at = $4
                        WHERE id = $1"#,
                )
                .bind(&o.id)
                .bind(&o.external_id)
                .bind(&o.signature)
                .bind(o.updated_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    pub async fn get(&self, id: &str) -> RepoResult<Option<Order>> {
        let row = self
            .db
            .timed(
                "order_get",
                sqlx::query("SELECT * FROM orders WHERE id = $1")
                    .bind(id)
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.as_ref().map(Self::map))
    }

    pub async fn get_by_key(&self, key: &str) -> RepoResult<Option<Order>> {
        let row = self
            .db
            .timed(
                "order_get_by_key",
                sqlx::query("SELECT * FROM orders WHERE idempotency_key = $1")
                    .bind(key)
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.as_ref().map(Self::map))
    }

    pub async fn get_by_signature(&self, sig: &str) -> RepoResult<Option<Order>> {
        let row = self
            .db
            .timed(
                "order_get_by_sig",
                sqlx::query(
                    "SELECT * FROM orders WHERE signature = $1 ORDER BY created_at DESC LIMIT 1",
                )
                .bind(sig)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.as_ref().map(Self::map))
    }

    /// All orders in non-terminal states (recovery + reconciliation input).
    pub async fn list_incomplete(&self) -> RepoResult<Vec<Order>> {
        let rows = self
            .db
            .timed(
                "orders_incomplete",
                sqlx::query(
                    r#"SELECT * FROM orders
                        WHERE status NOT IN ('filled','failed','cancelled','expired','reconciled')
                        ORDER BY created_at ASC LIMIT 5000"#,
                )
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(Self::map).collect())
    }

    pub async fn list_recent(&self, limit: i64) -> RepoResult<Vec<Order>> {
        let rows = self
            .db
            .timed(
                "orders_recent",
                sqlx::query("SELECT * FROM orders ORDER BY created_at DESC LIMIT $1")
                    .bind(limit.clamp(1, 1000))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(Self::map).collect())
    }

    pub async fn append_execution(&self, rec: &ExecutionRecord) -> RepoResult<()> {
        self.db
            .timed(
                "execution_append",
                sqlx::query(
                    r#"INSERT INTO executions (order_id, ts, kind, endpoint, latency_ms, ok, detail)
                       VALUES ($1,$2,$3,$4,$5,$6,$7)"#,
                )
                .bind(&rec.order_id)
                .bind(rec.ts)
                .bind(&rec.kind)
                .bind(&rec.endpoint)
                .bind(rec.latency_ms.map(|v| v as i64))
                .bind(rec.ok)
                .bind(&rec.detail)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Positions & trades
// ---------------------------------------------------------------------------

pub struct PositionRepo {
    db: Arc<Database>,
}

impl PositionRepo {
    pub fn new(db: Arc<Database>) -> Self {
        PositionRepo { db }
    }

    fn map(row: &PgRow) -> Position {
        let mut p = Position::new(
            row.try_get("id").unwrap_or_default(),
            TradeSource::parse(&row.try_get::<String, _>("source").unwrap_or_default())
                .unwrap_or(TradeSource::Manual),
            Venue::parse(&row.try_get::<String, _>("venue").unwrap_or_default())
                .unwrap_or(Venue::Paper),
            ExecutionMode::from_str(&row.try_get::<String, _>("mode").unwrap_or_default())
                .unwrap_or(ExecutionMode::Paper),
            row.try_get("symbol").unwrap_or_default(),
            row.try_get("symbol_display").unwrap_or_default(),
            row.try_get("quote_symbol").unwrap_or_default(),
        );
        p.status = PositionStatus::parse(&row.try_get::<String, _>("status").unwrap_or_default())
            .unwrap_or(PositionStatus::Open);
        p.qty = row.try_get("qty").unwrap_or(0.0);
        p.avg_entry = row.try_get("avg_entry").unwrap_or(0.0);
        p.cost_basis = row.try_get("cost_basis").unwrap_or(0.0);
        p.realized_quote = row.try_get("realized_quote").unwrap_or(0.0);
        p.last_mark = row.try_get("last_mark").unwrap_or(0.0);
        p.stop_loss = row.try_get("stop_loss").ok().flatten();
        p.take_profit = row.try_get("take_profit").ok().flatten();
        p.trailing_stop = row.try_get("trailing_stop").ok().flatten();
        p.trailing_high_water = row.try_get("trailing_high_water").ok().flatten();
        p.max_hold_secs = row.try_get("max_hold_secs").ok().flatten();
        p.entry_signature = row.try_get("entry_signature").ok().flatten();
        p.exit_signature = row.try_get("exit_signature").ok().flatten();
        p.entry_latency_ms = row
            .try_get::<Option<i64>, _>("entry_latency_ms")
            .ok()
            .flatten()
            .map(|v| v.max(0) as u64);
        p.copied_wallet = row.try_get("copied_wallet").ok().flatten();
        p.market_id = row.try_get("market_id").ok().flatten();
        p.outcome = row.try_get("outcome").ok().flatten();
        p.reason_closed = row.try_get("reason_closed").ok().flatten();
        if let Some(t) = ts(row, "opened_at") {
            p.opened_at = t;
        }
        if let Some(t) = ts(row, "updated_at") {
            p.updated_at = t;
        }
        p.closed_at = ts(row, "closed_at");
        p
    }

    /// Upsert the full position row (open, closing or terminal).
    pub async fn upsert(&self, p: &Position) -> RepoResult<()> {
        self.db
            .timed(
                "position_upsert",
                sqlx::query(
                    r#"INSERT INTO positions
                        (id, source, venue, mode, status, symbol, symbol_display, quote_symbol,
                         qty, avg_entry, cost_basis, realized_quote, last_mark,
                         stop_loss, take_profit, trailing_stop, trailing_high_water,
                         max_hold_secs, entry_signature, exit_signature, entry_latency_ms,
                         copied_wallet, market_id, outcome, reason_closed,
                         opened_at, updated_at, closed_at)
                       VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,
                               $18,$19,$20,$21,$22,$23,$24,$25,$26,$27,$28)
                       ON CONFLICT (id) DO UPDATE SET
                         status = EXCLUDED.status, qty = EXCLUDED.qty,
                         avg_entry = EXCLUDED.avg_entry, cost_basis = EXCLUDED.cost_basis,
                         realized_quote = EXCLUDED.realized_quote,
                         last_mark = EXCLUDED.last_mark, stop_loss = EXCLUDED.stop_loss,
                         take_profit = EXCLUDED.take_profit,
                         trailing_stop = EXCLUDED.trailing_stop,
                         trailing_high_water = EXCLUDED.trailing_high_water,
                         max_hold_secs = EXCLUDED.max_hold_secs,
                         exit_signature = COALESCE(EXCLUDED.exit_signature, positions.exit_signature),
                         reason_closed = EXCLUDED.reason_closed,
                         updated_at = EXCLUDED.updated_at,
                         closed_at = COALESCE(EXCLUDED.closed_at, positions.closed_at)"#,
                )
                .bind(&p.id)
                .bind(p.source.as_str())
                .bind(p.venue.as_str())
                .bind(p.mode.as_str())
                .bind(p.status.as_str())
                .bind(&p.symbol)
                .bind(&p.symbol_display)
                .bind(&p.quote_symbol)
                .bind(p.qty)
                .bind(p.avg_entry)
                .bind(p.cost_basis)
                .bind(p.realized_quote)
                .bind(p.last_mark)
                .bind(p.stop_loss)
                .bind(p.take_profit)
                .bind(p.trailing_stop)
                .bind(p.trailing_high_water)
                .bind(p.max_hold_secs)
                .bind(&p.entry_signature)
                .bind(&p.exit_signature)
                .bind(p.entry_latency_ms.map(|v| v as i64))
                .bind(&p.copied_wallet)
                .bind(&p.market_id)
                .bind(&p.outcome)
                .bind(&p.reason_closed)
                .bind(p.opened_at)
                .bind(p.updated_at)
                .bind(p.closed_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Live positions (open or closing) — the restart-recovery input.
    pub async fn list_open(&self) -> RepoResult<Vec<Position>> {
        let rows = self
            .db
            .timed(
                "positions_open",
                sqlx::query(
                    r#"SELECT * FROM positions WHERE status IN ('open','closing')
                        ORDER BY opened_at ASC LIMIT 5000"#,
                )
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(Self::map).collect())
    }

    pub async fn get(&self, id: &str) -> RepoResult<Option<Position>> {
        let row = self
            .db
            .timed(
                "position_get",
                sqlx::query("SELECT * FROM positions WHERE id = $1")
                    .bind(id)
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.as_ref().map(Self::map))
    }

    pub async fn list_recent(&self, limit: i64) -> RepoResult<Vec<Position>> {
        let rows = self
            .db
            .timed(
                "positions_recent",
                sqlx::query("SELECT * FROM positions ORDER BY updated_at DESC LIMIT $1")
                    .bind(limit.clamp(1, 1000))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(Self::map).collect())
    }
}

pub struct TradeRepo {
    db: Arc<Database>,
}

impl TradeRepo {
    pub fn new(db: Arc<Database>) -> Self {
        TradeRepo { db }
    }

    /// Append a fill. Idempotent on the app-assigned trade id: a retry after
    /// an ambiguous failure updates rather than duplicates.
    pub async fn append(&self, t: &Trade) -> RepoResult<()> {
        self.db
            .timed(
                "trade_append",
                sqlx::query(
                    r#"INSERT INTO trades
                        (id, ts, source, venue, mode, side, symbol, symbol_display,
                         amount_in, amount_out, quote_symbol, price, fee, slippage_bps,
                         signature, position_id, note, latency_ms)
                       VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)
                       ON CONFLICT (id) DO NOTHING"#,
                )
                .bind(&t.id)
                .bind(t.ts)
                .bind(t.source.as_str())
                .bind(t.venue.as_str())
                .bind(t.mode.as_str())
                .bind(t.side.as_str())
                .bind(&t.symbol)
                .bind(&t.symbol_display)
                .bind(t.amount_in)
                .bind(t.amount_out)
                .bind(&t.quote_symbol)
                .bind(t.price)
                .bind(t.fee)
                .bind(t.slippage_bps as i64)
                .bind(&t.signature)
                .bind(&t.position_id)
                .bind(&t.note)
                .bind(t.latency_ms.map(|v| v as i64))
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    pub async fn list_recent(&self, limit: i64) -> RepoResult<Vec<Trade>> {
        let rows = self
            .db
            .timed(
                "trades_recent",
                sqlx::query("SELECT * FROM trades ORDER BY ts DESC LIMIT $1")
                    .bind(limit.clamp(1, 1000))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(Self::map).collect())
    }

    /// Chronological fill history of one position — the authoritative
    /// execution data PnL reconstruction replays (Prompt 2 §K).
    pub async fn list_for_position(&self, position_id: &str) -> RepoResult<Vec<Trade>> {
        let rows = self
            .db
            .timed(
                "trades_for_position",
                sqlx::query("SELECT * FROM trades WHERE position_id = $1 ORDER BY ts ASC")
                    .bind(position_id)
                    .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(Self::map).collect())
    }

    fn map(row: &PgRow) -> Trade {
        Trade {
            id: row.try_get("id").unwrap_or_default(),
            ts: ts(row, "ts").unwrap_or_else(Utc::now),
            source: TradeSource::parse(&row.try_get::<String, _>("source").unwrap_or_default())
                .unwrap_or(TradeSource::Manual),
            venue: Venue::parse(&row.try_get::<String, _>("venue").unwrap_or_default())
                .unwrap_or(Venue::Paper),
            mode: ExecutionMode::from_str(&row.try_get::<String, _>("mode").unwrap_or_default())
                .unwrap_or(ExecutionMode::Paper),
            side: PositionSide::parse(&row.try_get::<String, _>("side").unwrap_or_default())
                .unwrap_or(PositionSide::Long),
            symbol: row.try_get("symbol").unwrap_or_default(),
            symbol_display: row.try_get("symbol_display").unwrap_or_default(),
            amount_in: row.try_get("amount_in").unwrap_or(0.0),
            amount_out: row.try_get("amount_out").unwrap_or(0.0),
            quote_symbol: row.try_get("quote_symbol").unwrap_or_default(),
            price: row.try_get("price").unwrap_or(0.0),
            fee: row.try_get("fee").unwrap_or(0.0),
            slippage_bps: row
                .try_get::<Option<i64>, _>("slippage_bps")
                .ok()
                .flatten()
                .unwrap_or(0)
                .max(0) as u64,
            signature: row.try_get("signature").ok().flatten(),
            position_id: row.try_get("position_id").ok().flatten(),
            note: row.try_get("note").ok().flatten(),
            latency_ms: row
                .try_get::<Option<i64>, _>("latency_ms")
                .ok()
                .flatten()
                .map(|v| v.max(0) as u64),
        }
    }
}

// ---------------------------------------------------------------------------
// Dedup (restart-safe) + idempotency
// ---------------------------------------------------------------------------

pub struct DedupRepo {
    db: Arc<Database>,
}

impl DedupRepo {
    pub fn new(db: Arc<Database>) -> Self {
        DedupRepo { db }
    }

    /// Atomic first-arrival check. `Ok(true)` = this call is the FIRST for
    /// (namespace, key) — the caller may act. `Ok(false)` = already seen.
    pub async fn mark(&self, ns: &str, key: &str, ttl: std::time::Duration) -> RepoResult<bool> {
        let expires = if ttl.is_zero() {
            None
        } else {
            Some(Utc::now() + chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::days(7)))
        };
        let res = self
            .db
            .timed(
                "dedup_mark",
                sqlx::query(
                    r#"INSERT INTO dedup_keys (namespace, key, expires_at)
                       VALUES ($1, $2, $3)
                       ON CONFLICT (namespace, key) DO NOTHING"#,
                )
                .bind(ns)
                .bind(key)
                .bind(expires)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Remove a key (used when a transaction provably did not land and the
    /// operation may legitimately be retried).
    pub async fn forget(&self, ns: &str, key: &str) -> RepoResult<()> {
        self.db
            .timed(
                "dedup_forget",
                sqlx::query("DELETE FROM dedup_keys WHERE namespace = $1 AND key = $2")
                    .bind(ns)
                    .bind(key)
                    .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    pub async fn exists(&self, ns: &str, key: &str) -> RepoResult<bool> {
        let row = self
            .db
            .timed(
                "dedup_exists",
                sqlx::query("SELECT 1 AS one FROM dedup_keys WHERE namespace = $1 AND key = $2")
                    .bind(ns)
                    .bind(key)
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.is_some())
    }

    /// Retention: drop expired keys. Returns rows deleted.
    pub async fn cleanup_expired(&self) -> RepoResult<u64> {
        let res = self
            .db
            .timed(
                "dedup_cleanup",
                sqlx::query(
                    "DELETE FROM dedup_keys WHERE expires_at IS NOT NULL AND expires_at < now()",
                )
                .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected())
    }
}

pub struct IdempotencyRepo {
    db: Arc<Database>,
}

impl IdempotencyRepo {
    pub fn new(db: Arc<Database>) -> Self {
        IdempotencyRepo { db }
    }

    /// Reserve (scope, key). `Ok(true)` = newly reserved (proceed);
    /// `Ok(false)` = already consumed (duplicate).
    pub async fn try_consume(&self, scope: &str, key: &str) -> RepoResult<bool> {
        let res = self
            .db
            .timed(
                "idem_consume",
                sqlx::query(
                    r#"INSERT INTO idempotency_keys (scope, key, consumed_at)
                       VALUES ($1, $2, now())
                       ON CONFLICT (scope, key) DO NOTHING"#,
                )
                .bind(scope)
                .bind(key)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected() == 1)
    }

    pub async fn record_response(
        &self,
        scope: &str,
        key: &str,
        resp: &serde_json::Value,
    ) -> RepoResult<()> {
        self.db
            .timed(
                "idem_response",
                sqlx::query(
                    "UPDATE idempotency_keys SET response = $3 WHERE scope = $1 AND key = $2",
                )
                .bind(scope)
                .bind(key)
                .bind(resp)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    pub async fn cleanup_older_than(&self, age: chrono::Duration) -> RepoResult<u64> {
        let cutoff = Utc::now() - age;
        let res = self
            .db
            .timed(
                "idem_cleanup",
                sqlx::query("DELETE FROM idempotency_keys WHERE created_at < $1")
                    .bind(cutoff)
                    .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected())
    }
}

// ---------------------------------------------------------------------------
// Transactions (on-chain bookkeeping)
// ---------------------------------------------------------------------------

pub struct TransactionRepo {
    db: Arc<Database>,
}

impl TransactionRepo {
    pub fn new(db: Arc<Database>) -> Self {
        TransactionRepo { db }
    }

    /// Insert-if-absent; a repeat submit of the same signature is a no-op.
    /// Persist one money-moving execution attempt. `chain` is `solana` or
    /// `polymarket`; `signer`/`venue`/`attempts` carry the attribution the
    /// reconciliation engine and operators need (§E). Idempotent: re-recording
    /// the same signature never overwrites existing state.
    /// Current persisted status of one transaction claim (`submitted`,
    /// `confirmed`, `finalized`, `failed`, `not_found`) — the LOCAL side of
    /// the execution reconciliation matrix.
    pub async fn get_status(&self, signature: &str) -> RepoResult<Option<String>> {
        let row: Option<(String,)> = self
            .db
            .timed(
                "tx_get_status",
                sqlx::query_as("SELECT status FROM transactions WHERE signature = $1")
                    .bind(signature)
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.map(|r| r.0))
    }

    pub async fn record_submitted(
        &self,
        chain: &str,
        signature: &str,
        order_id: Option<&str>,
        signer: Option<&str>,
        venue: Option<&str>,
        attempts: i32,
    ) -> RepoResult<bool> {
        // Cross-replica truth for `attempts`: while the row is still
        // 'submitted', a re-recording replica MAXES the broadcast-attempt
        // count and fills in attribution it has but the row lacks (COALESCE
        // never overwrites existing values). Once the row reached a terminal
        // status the WHERE guard makes the conflict a no-op — durable history
        // is immutable (§Y). `xmax = 0` distinguishes a fresh INSERT from an
        // UPDATE-or-nothing, so the return value stays "first recording".
        let row: Option<(bool,)> = self
            .db
            .timed(
                "tx_submitted",
                sqlx::query_as(
                    r#"INSERT INTO transactions
                           (signature, chain, order_id, status, signer, venue, attempts)
                       VALUES ($1, $2, $3, 'submitted', $4, $5, $6)
                       ON CONFLICT (signature) DO UPDATE
                           SET attempts = GREATEST(transactions.attempts, EXCLUDED.attempts),
                               order_id = COALESCE(transactions.order_id, EXCLUDED.order_id),
                               signer   = COALESCE(transactions.signer,   EXCLUDED.signer),
                               venue    = COALESCE(transactions.venue,    EXCLUDED.venue)
                         WHERE transactions.status = 'submitted'
                       RETURNING (xmax = 0)"#,
                )
                .bind(signature)
                .bind(chain)
                .bind(order_id)
                .bind(signer)
                .bind(venue)
                .bind(attempts)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.map(|r| r.0).unwrap_or(false))
    }

    /// The order attributed to one signature (0006 attribution), if known.
    /// Used by the startup gate to map `transaction` claims to symbols.
    pub async fn get_order_id(&self, signature: &str) -> RepoResult<Option<String>> {
        let row: Option<(Option<String>,)> = self
            .db
            .timed(
                "tx_get_order_id",
                sqlx::query_as("SELECT order_id FROM transactions WHERE signature = $1")
                    .bind(signature)
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.and_then(|r| r.0))
    }

    /// Broadcast-attempt count persisted for one signature (cross-replica max).
    pub async fn get_attempts(&self, signature: &str) -> RepoResult<Option<i32>> {
        let row: Option<(i32,)> = self
            .db
            .timed(
                "tx_get_attempts",
                sqlx::query_as("SELECT attempts FROM transactions WHERE signature = $1")
                    .bind(signature)
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.map(|r| r.0))
    }

    pub async fn set_status(
        &self,
        signature: &str,
        status: &str,
        slot: Option<i64>,
        error: Option<&str>,
    ) -> RepoResult<()> {
        let landed = matches!(status, "confirmed" | "finalized");
        self.db
            .timed(
                "tx_status",
                sqlx::query(
                    r#"UPDATE transactions SET status = $2, landed = $3,
                              slot = COALESCE($4, slot), error = $5,
                              confirmed_at = CASE WHEN $3 AND confirmed_at IS NULL
                                                  THEN now() ELSE confirmed_at END
                        WHERE signature = $1"#,
                )
                .bind(signature)
                .bind(status)
                .bind(landed)
                .bind(slot)
                .bind(error)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Transactions whose final state was never observed and that were
    /// submitted before `cutoff` (a confirmation could have landed by now).
    pub async fn list_unresolved_before(
        &self,
        cutoff: DateTime<Utc>,
        limit: i64,
    ) -> RepoResult<Vec<(String, Option<String>)>> {
        let rows = self
            .db
            .timed(
                "tx_unresolved",
                sqlx::query(
                    r#"SELECT signature, order_id FROM transactions
                        WHERE status IN ('submitted','confirmed')
                          AND submitted_at < $1
                        ORDER BY submitted_at ASC LIMIT $2"#,
                )
                .bind(cutoff)
                .bind(limit.clamp(1, 1000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    r.try_get::<String, _>("signature").unwrap_or_default(),
                    r.try_get("order_id").ok(),
                )
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Execution intents (write-ahead journal — crash point C)
// ---------------------------------------------------------------------------

/// One pre-broadcast intent row (§I crash point C: durable evidence that a
/// money-moving transaction was ABOUT to leave the process).
#[derive(Debug, Clone, serde::Serialize)]
pub struct IntentRecord {
    pub intent_id: String,
    pub module: String,
    pub symbol: String,
    pub wallet: String,
    pub side: String,
    /// Human-readable quantity (string keeps token decimals out of the schema).
    pub qty: String,
    /// pending | submitted | abandoned
    pub status: String,
    pub signature: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct IntentRepo {
    db: Arc<Database>,
}

impl IntentRepo {
    pub fn new(db: Arc<Database>) -> Self {
        IntentRepo { db }
    }

    /// Journal an intent BEFORE broadcast. Idempotent on `intent_id`
    /// (ON CONFLICT DO NOTHING): a retry of the same intent never rewrites
    /// the original timestamp — age is what makes an orphan ambiguous.
    pub async fn record(&self, rec: &IntentRecord) -> RepoResult<()> {
        self.db
            .timed(
                "intent_record",
                sqlx::query(
                    r#"INSERT INTO execution_intents
                           (intent_id, module, symbol, wallet, side, qty)
                       VALUES ($1, $2, $3, $4, $5, $6)
                       ON CONFLICT (intent_id) DO NOTHING"#,
                )
                .bind(&rec.intent_id)
                .bind(&rec.module)
                .bind(&rec.symbol)
                .bind(&rec.wallet)
                .bind(&rec.side)
                .bind(&rec.qty)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// The broadcast produced `signature`: the intent is no longer ambiguous.
    /// Only pending intents transition (terminal rows are immutable).
    pub async fn link(&self, intent_id: &str, signature: &str) -> RepoResult<()> {
        self.db
            .timed(
                "intent_link",
                sqlx::query(
                    r#"UPDATE execution_intents
                          SET status = 'submitted', signature = $2, updated_at = now()
                        WHERE intent_id = $1 AND status = 'pending'"#,
                )
                .bind(intent_id)
                .bind(signature)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// The attempt provably never broadcast (terminal error, no signature).
    pub async fn abandon(&self, intent_id: &str) -> RepoResult<()> {
        self.db
            .timed(
                "intent_abandon",
                sqlx::query(
                    r#"UPDATE execution_intents
                          SET status = 'abandoned', updated_at = now()
                        WHERE intent_id = $1 AND status = 'pending'"#,
                )
                .bind(intent_id)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    pub async fn get(&self, intent_id: &str) -> RepoResult<Option<IntentRecord>> {
        let row = self
            .db
            .timed(
                "intent_get",
                sqlx::query("SELECT * FROM execution_intents WHERE intent_id = $1")
                    .bind(intent_id)
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.as_ref().map(Self::map))
    }

    /// Pending intents older than `cutoff`: orphans whose outcome is unknown
    /// (crash between record and link/abandon). Ordered oldest first.
    pub async fn list_orphaned(
        &self,
        cutoff: DateTime<Utc>,
        limit: i64,
    ) -> RepoResult<Vec<IntentRecord>> {
        let rows = self
            .db
            .timed(
                "intent_orphans",
                sqlx::query(
                    r#"SELECT * FROM execution_intents
                        WHERE status = 'pending' AND created_at < $1
                        ORDER BY created_at ASC LIMIT $2"#,
                )
                .bind(cutoff)
                .bind(limit.clamp(1, 1000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(Self::map).collect())
    }

    fn map(row: &PgRow) -> IntentRecord {
        IntentRecord {
            intent_id: row.try_get("intent_id").unwrap_or_default(),
            module: row.try_get("module").unwrap_or_default(),
            symbol: row.try_get("symbol").unwrap_or_default(),
            wallet: row.try_get("wallet").unwrap_or_default(),
            side: row.try_get("side").unwrap_or_default(),
            qty: row.try_get("qty").unwrap_or_default(),
            status: row.try_get("status").unwrap_or_default(),
            signature: row.try_get("signature").ok().flatten(),
            created_at: ts(row, "created_at").unwrap_or_else(Utc::now),
        }
    }
}

// ---------------------------------------------------------------------------
// Audit trail (append-only, hash-chained)
// ---------------------------------------------------------------------------

/// One audit row as stored (and as returned by the read APIs).
#[derive(Debug, Clone, serde::Serialize)]
pub struct AuditEntry {
    pub id: i64,
    pub ts: DateTime<Utc>,
    pub actor: String,
    pub action: String,
    pub target: Option<String>,
    pub outcome: String,
    pub detail: serde_json::Value,
    pub prev_hash: String,
    pub hash: String,
}

pub struct AuditRepo {
    db: Arc<Database>,
}

impl AuditRepo {
    pub fn new(db: Arc<Database>) -> Self {
        AuditRepo { db }
    }

    /// Genesis value for the first chain link.
    pub const GENESIS: &'static str = "genesis";

    /// Compute the chain hash over the canonical row content.
    pub fn chain_hash(
        prev: &str,
        ts: &DateTime<Utc>,
        actor: &str,
        action: &str,
        target: Option<&str>,
        outcome: &str,
        detail: &serde_json::Value,
    ) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(prev.as_bytes());
        h.update(b"|");
        h.update(ts.to_rfc3339().as_bytes());
        h.update(b"|");
        h.update(actor.as_bytes());
        h.update(b"|");
        h.update(action.as_bytes());
        h.update(b"|");
        h.update(target.unwrap_or("").as_bytes());
        h.update(b"|");
        h.update(outcome.as_bytes());
        h.update(b"|");
        // serde_json's default serialization is deterministic for a given
        // Value (map keys sorted by the serde_json feature "preserve_order"
        // OFF — the default — so BTreeMap ordering applies).
        h.update(detail.to_string().as_bytes());
        hex::encode(h.finalize())
    }

    /// Append with the hash chain computed inside a transaction serialized
    /// by a Postgres transaction-scoped advisory lock — a row lock on the
    /// head entry is NOT sufficient under READ COMMITTED (see the comment
    /// inside `append` and the `audit_chain_survives_concurrent_appends`
    /// regression test).
    pub async fn append(
        &self,
        actor: &str,
        action: &str,
        target: Option<&str>,
        outcome: &str,
        detail: &serde_json::Value,
    ) -> RepoResult<AuditEntry> {
        let ts = Utc::now();
        let mut tx = self.db.pool().begin().await.map_err(TimedDbError::Error)?;
        // Serialize chain appends across ALL connections, processes and
        // replicas with a transaction-scoped advisory lock (held until
        // commit). A `FOR UPDATE` on the current head row is NOT sufficient:
        // under READ COMMITTED a blocked writer's snapshot never sees the
        // row the winner inserted after the scan started (EvalPlanQual only
        // rechecks the locked row itself), so the loser would append from a
        // stale head and fork the chain. Regression-tested by
        // `audit_chain_survives_concurrent_appends` in db_integration.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('audit_events_chain'))")
            .execute(&mut *tx)
            .await
            .map_err(TimedDbError::Error)?;
        let head: Option<String> =
            sqlx::query("SELECT hash FROM audit_events ORDER BY id DESC LIMIT 1")
                .fetch_optional(&mut *tx)
                .await
                .map_err(TimedDbError::Error)?
                .and_then(|r| r.try_get("hash").ok());
        let prev_hash = head.unwrap_or_else(|| Self::GENESIS.to_string());
        // Canonicalize the timestamp to MICROSECONDS before hashing: the
        // timestamptz column truncates to µs, so hashing a ns-precision
        // `Utc::now()` would make the stored row unverifiable on clocks with
        // ns granularity (verify recomputes from the truncated value).
        let ts = chrono::DateTime::from_timestamp_micros(ts.timestamp_micros()).unwrap_or(ts);
        let hash = Self::chain_hash(&prev_hash, &ts, actor, action, target, outcome, detail);
        let row = sqlx::query(
            r#"INSERT INTO audit_events (ts, actor, action, target, outcome, detail, prev_hash, hash)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
               RETURNING id"#,
        )
        .bind(ts)
        .bind(actor)
        .bind(action)
        .bind(target)
        .bind(outcome)
        .bind(detail)
        .bind(&prev_hash)
        .bind(&hash)
        .fetch_one(&mut *tx)
        .await
        .map_err(TimedDbError::Error)?;
        let id: i64 = row.try_get("id").map_err(TimedDbError::Error)?;
        tx.commit().await.map_err(TimedDbError::Error)?;
        Ok(AuditEntry {
            id,
            ts,
            actor: actor.to_string(),
            action: action.to_string(),
            target: target.map(str::to_string),
            outcome: outcome.to_string(),
            detail: detail.clone(),
            prev_hash,
            hash,
        })
    }

    /// Total rows in the durable log.
    pub async fn count(&self) -> RepoResult<i64> {
        let row = self
            .db
            .timed(
                "audit_count",
                sqlx::query("SELECT COUNT(*) AS n FROM audit_events").fetch_one(self.db.pool()),
            )
            .await?;
        Ok(row.try_get::<i64, _>("n").unwrap_or(0))
    }

    pub async fn list_recent(&self, limit: i64) -> RepoResult<Vec<AuditEntry>> {
        let rows = self
            .db
            .timed(
                "audit_list",
                sqlx::query("SELECT * FROM audit_events ORDER BY id DESC LIMIT $1")
                    .bind(limit.clamp(1, 1000))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(Self::map).collect())
    }

    /// Walk the whole chain from genesis, recomputing every hash.
    /// Returns `Ok(None)` when intact, or `Ok(Some(broken_at_id))` when a
    /// link does not match (tamper evidence).
    pub async fn verify_chain(&self) -> RepoResult<Option<i64>> {
        let rows = self
            .db
            .timed(
                "audit_verify",
                sqlx::query("SELECT * FROM audit_events ORDER BY id ASC LIMIT 100000")
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let mut prev = Self::GENESIS.to_string();
        for row in &rows {
            let entry = Self::map(row);
            if entry.prev_hash != prev {
                return Ok(Some(entry.id));
            }
            let recomputed = Self::chain_hash(
                &entry.prev_hash,
                &entry.ts,
                &entry.actor,
                &entry.action,
                entry.target.as_deref(),
                &entry.outcome,
                &entry.detail,
            );
            if recomputed != entry.hash {
                return Ok(Some(entry.id));
            }
            prev = entry.hash.clone();
        }
        Ok(None)
    }

    fn map(row: &PgRow) -> AuditEntry {
        AuditEntry {
            id: row.try_get("id").unwrap_or(0),
            ts: ts(row, "ts").unwrap_or_else(Utc::now),
            actor: row.try_get("actor").unwrap_or_default(),
            action: row.try_get("action").unwrap_or_default(),
            target: row.try_get("target").ok().flatten(),
            outcome: row.try_get("outcome").unwrap_or_default(),
            detail: row.try_get("detail").unwrap_or(serde_json::json!({})),
            prev_hash: row.try_get("prev_hash").unwrap_or_default(),
            hash: row.try_get("hash").unwrap_or_default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Risk events, system events, config versions
// ---------------------------------------------------------------------------

pub struct RiskEventRepo {
    db: Arc<Database>,
}

impl RiskEventRepo {
    pub fn new(db: Arc<Database>) -> Self {
        RiskEventRepo { db }
    }

    pub async fn append(
        &self,
        module: &str,
        kind: &str,
        symbol: Option<&str>,
        reason: &str,
        snapshot: &serde_json::Value,
    ) -> RepoResult<()> {
        self.db
            .timed(
                "risk_event_append",
                sqlx::query(
                    r#"INSERT INTO risk_events (module, kind, symbol, reason, snapshot)
                       VALUES ($1,$2,$3,$4,$5)"#,
                )
                .bind(module)
                .bind(kind)
                .bind(symbol)
                .bind(reason)
                .bind(snapshot)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    pub async fn list_recent(&self, limit: i64) -> RepoResult<Vec<serde_json::Value>> {
        let rows = self
            .db
            .timed(
                "risk_events_list",
                sqlx::query("SELECT * FROM risk_events ORDER BY id DESC LIMIT $1")
                    .bind(limit.clamp(1, 1000))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.try_get::<i64, _>("id").unwrap_or(0),
                    "ts": ts(r, "ts").map(|t| t.to_rfc3339()).unwrap_or_default(),
                    "module": r.try_get::<String, _>("module").unwrap_or_default(),
                    "kind": r.try_get::<String, _>("kind").unwrap_or_default(),
                    "symbol": r.try_get::<Option<String>, _>("symbol").ok().flatten(),
                    "reason": r.try_get::<String, _>("reason").unwrap_or_default(),
                    "snapshot": r.try_get::<serde_json::Value, _>("snapshot").unwrap_or(serde_json::json!({})),
                })
            })
            .collect())
    }
}

pub struct SystemEventRepo {
    db: Arc<Database>,
}

impl SystemEventRepo {
    pub fn new(db: Arc<Database>) -> Self {
        SystemEventRepo { db }
    }

    /// Append one structured incident row. `severity` must satisfy the
    /// table CHECK (debug/info/warn/error/fatal).
    pub async fn append(
        &self,
        kind: &str,
        module: Option<&str>,
        severity: &str,
        message: &str,
        payload: &serde_json::Value,
    ) -> RepoResult<()> {
        self.db
            .timed(
                "system_event_append",
                sqlx::query(
                    r#"INSERT INTO system_events (kind, module, severity, message, payload)
                       VALUES ($1,$2,$3,$4,$5)"#,
                )
                .bind(kind)
                .bind(module)
                .bind(severity)
                .bind(message.chars().take(2000).collect::<String>())
                .bind(payload)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    pub async fn list_recent(&self, limit: i64) -> RepoResult<Vec<serde_json::Value>> {
        let rows = self
            .db
            .timed(
                "system_events_list",
                sqlx::query("SELECT * FROM system_events ORDER BY id DESC LIMIT $1")
                    .bind(limit.clamp(1, 500))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.try_get::<i64, _>("id").unwrap_or(0),
                    "ts": ts(r, "ts").map(|t| t.to_rfc3339()).unwrap_or_default(),
                    "kind": r.try_get::<String, _>("kind").unwrap_or_default(),
                    "module": r.try_get::<Option<String>, _>("module").ok().flatten(),
                    "severity": r.try_get::<String, _>("severity").unwrap_or_default(),
                    "message": r.try_get::<String, _>("message").unwrap_or_default(),
                    "payload": r.try_get::<serde_json::Value, _>("payload").unwrap_or(serde_json::json!({})),
                })
            })
            .collect())
    }

    /// Retention: delete events older than `age`.
    pub async fn delete_older_than(&self, age: chrono::Duration) -> RepoResult<u64> {
        let cutoff = Utc::now() - age;
        let res = self
            .db
            .timed(
                "system_events_cleanup",
                sqlx::query("DELETE FROM system_events WHERE ts < $1")
                    .bind(cutoff)
                    .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected())
    }
}

pub struct ConfigVersionRepo {
    db: Arc<Database>,
}

impl ConfigVersionRepo {
    pub fn new(db: Arc<Database>) -> Self {
        ConfigVersionRepo { db }
    }

    /// Record a config snapshot if this exact sha256 is not already the
    /// head. `Ok(true)` = newly recorded.
    pub async fn record(
        &self,
        sha256: &str,
        snapshot: &serde_json::Value,
        source: &str,
        note: Option<&str>,
    ) -> RepoResult<bool> {
        let head: Option<String> = self
            .db
            .timed(
                "config_head",
                sqlx::query("SELECT sha256 FROM config_versions ORDER BY version DESC LIMIT 1")
                    .fetch_optional(self.db.pool()),
            )
            .await?
            .and_then(|r| r.try_get("sha256").ok());
        if head.as_deref() == Some(sha256) {
            return Ok(false);
        }
        let res = self
            .db
            .timed(
                "config_record",
                sqlx::query(
                    r#"INSERT INTO config_versions (source, sha256, snapshot, note)
                       VALUES ($1,$2,$3,$4)"#,
                )
                .bind(source)
                .bind(sha256)
                .bind(snapshot)
                .bind(note)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected() == 1)
    }

    pub async fn list_recent(&self, limit: i64) -> RepoResult<Vec<serde_json::Value>> {
        let rows = self
            .db
            .timed(
                "config_list",
                sqlx::query("SELECT * FROM config_versions ORDER BY version DESC LIMIT $1")
                    .bind(limit.clamp(1, 100))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "version": r.try_get::<i64, _>("version").unwrap_or(0),
                    "sha256": r.try_get::<String, _>("sha256").unwrap_or_default(),
                    "source": r.try_get::<String, _>("source").unwrap_or_default(),
                    "note": r.try_get::<Option<String>, _>("note").ok().flatten(),
                    "applied_at": ts(r, "applied_at").map(|t| t.to_rfc3339()).unwrap_or_default(),
                })
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// API key administration (hashes only — plaintext never reaches the DB)
// ---------------------------------------------------------------------------

/// One stored API principal.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ApiKeyRow {
    pub id: String,
    pub key_hash: String,
    pub label: String,
    pub role: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

pub struct ApiKeyRepo {
    db: Arc<Database>,
}

impl ApiKeyRepo {
    pub fn new(db: Arc<Database>) -> Self {
        ApiKeyRepo { db }
    }

    /// Insert a new key hash. `Ok(false)` when the hash already exists.
    pub async fn insert(&self, key_hash: &str, label: &str, role: &str) -> RepoResult<bool> {
        let id = uuid::Uuid::new_v4();
        let res = self
            .db
            .timed(
                "apikey_insert",
                sqlx::query(
                    r#"INSERT INTO api_keys (id, key_hash, label, role)
                       VALUES ($1,$2,$3,$4)
                       ON CONFLICT (key_hash) DO NOTHING"#,
                )
                .bind(id)
                .bind(key_hash)
                .bind(label)
                .bind(role)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected() == 1)
    }

    pub async fn set_enabled(&self, key_hash: &str, enabled: bool) -> RepoResult<()> {
        self.db
            .timed(
                "apikey_enable",
                sqlx::query("UPDATE api_keys SET enabled = $2 WHERE key_hash = $1")
                    .bind(key_hash)
                    .bind(enabled)
                    .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    pub async fn touch_last_used(&self, key_hash: &str) -> RepoResult<()> {
        // Best-effort, throttled by the caller (once per minute per key is
        // plenty); failures are irrelevant to authorization.
        self.db
            .timed(
                "apikey_touch",
                sqlx::query("UPDATE api_keys SET last_used_at = now() WHERE key_hash = $1")
                    .bind(key_hash)
                    .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    pub async fn list(&self) -> RepoResult<Vec<ApiKeyRow>> {
        let rows = self
            .db
            .timed(
                "apikey_list",
                sqlx::query("SELECT * FROM api_keys ORDER BY created_at ASC")
                    .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| ApiKeyRow {
                id: r
                    .try_get::<uuid::Uuid, _>("id")
                    .map(|u| u.to_string())
                    .unwrap_or_default(),
                key_hash: r.try_get("key_hash").unwrap_or_default(),
                label: r.try_get("label").unwrap_or_default(),
                role: r.try_get("role").unwrap_or_default(),
                enabled: r.try_get("enabled").unwrap_or(true),
                created_at: ts(r, "created_at").unwrap_or_else(Utc::now),
                last_used_at: ts(r, "last_used_at"),
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Wallet registry
// ---------------------------------------------------------------------------

pub struct WalletRepo {
    db: Arc<Database>,
}

impl WalletRepo {
    pub fn new(db: Arc<Database>) -> Self {
        WalletRepo { db }
    }

    /// Register (or re-label) a managed address. Addresses are public keys —
    /// no secret material ever reaches this table.
    pub async fn upsert(
        &self,
        label: &str,
        chain: &str,
        address: &str,
        kind: &str,
    ) -> RepoResult<()> {
        let id = uuid::Uuid::new_v4();
        self.db
            .timed(
                "wallet_upsert",
                sqlx::query(
                    r#"INSERT INTO wallets (id, label, chain, address, kind)
                       VALUES ($1,$2,$3,$4,$5)
                       ON CONFLICT (address) DO UPDATE SET
                         label = EXCLUDED.label, kind = EXCLUDED.kind"#,
                )
                .bind(id)
                .bind(label)
                .bind(chain)
                .bind(address)
                .bind(kind)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    pub async fn list(&self) -> RepoResult<Vec<serde_json::Value>> {
        let rows = self
            .db
            .timed(
                "wallet_list",
                sqlx::query("SELECT * FROM wallets ORDER BY created_at ASC")
                    .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "label": r.try_get::<String, _>("label").unwrap_or_default(),
                    "chain": r.try_get::<String, _>("chain").unwrap_or_default(),
                    "address": r.try_get::<String, _>("address").unwrap_or_default(),
                    "kind": r.try_get::<String, _>("kind").unwrap_or_default(),
                    "created_at": ts(r, "created_at").map(|t| t.to_rfc3339()).unwrap_or_default(),
                })
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Reconciliation queue + worker checkpoints
// ---------------------------------------------------------------------------

/// One due reconciliation item claimed by a worker.
#[derive(Debug, Clone)]
pub struct ReconItem {
    pub kind: String,
    pub subject: String,
    pub attempts: i32,
    pub max_attempts: i32,
}

pub struct ReconRepo {
    db: Arc<Database>,
}

impl ReconRepo {
    pub fn new(db: Arc<Database>) -> Self {
        ReconRepo { db }
    }

    /// Enqueue (or refresh the due time of) a subject needing verification.
    /// Unresolved (pending / in_progress / failed) claim counts per kind —
    /// used by the startup reconciliation gate, `/api/status`, the metrics
    /// sampler and Telegram. Low cardinality: kinds are a fixed set.
    /// `active_only = true` counts pending/in_progress claims (the startup
    /// gate + module-blocking input); `false` also includes claims parked in
    /// `failed` for operators (status surfaces).
    /// The individual unresolved claims as `(kind, subject)` pairs, ordered
    /// for deterministic output. `active_only` = pending/in_progress (what
    /// the startup gate acts on); otherwise every non-resolved row (incl.
    /// parked `failed`). The per-symbol gate uses this to attribute claims
    /// to symbols instead of disabling whole modules (§H).
    pub async fn list_unresolved_items(
        &self,
        active_only: bool,
        limit: i64,
    ) -> RepoResult<Vec<(String, String)>> {
        let sql = if active_only {
            r#"SELECT kind, subject FROM reconciliation_state
                WHERE status IN ('pending', 'in_progress')
                ORDER BY kind, subject LIMIT $1"#
        } else {
            r#"SELECT kind, subject FROM reconciliation_state
                WHERE status <> 'resolved'
                ORDER BY kind, subject LIMIT $1"#
        };
        let rows = self
            .db
            .timed(
                "recon_unresolved_items",
                sqlx::query(sql)
                    .bind(limit.clamp(1, 5000))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    r.try_get::<String, _>("kind").unwrap_or_default(),
                    r.try_get::<String, _>("subject").unwrap_or_default(),
                )
            })
            .collect())
    }

    pub async fn unresolved_counts(&self, active_only: bool) -> RepoResult<Vec<(String, i64)>> {
        let sql = if active_only {
            r#"SELECT kind, COUNT(*)::bigint
                 FROM reconciliation_state
                WHERE status IN ('pending', 'in_progress')
                GROUP BY kind"#
        } else {
            r#"SELECT kind, COUNT(*)::bigint
                 FROM reconciliation_state
                WHERE status <> 'resolved'
                GROUP BY kind"#
        };
        let rows: Vec<(String, i64)> = self
            .db
            .timed(
                "recon_unresolved_counts",
                sqlx::query_as(sql).fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows)
    }

    /// Re-arm a RESOLVED claim for periodic re-verification (live position
    /// truth is re-checked on a schedule). Parked (`failed`) claims are left
    /// alone — they belong to the operator queue — and active claims keep
    /// their backoff.
    pub async fn reopen_resolved(&self, kind: &str, subject: &str) -> RepoResult<bool> {
        let res = self
            .db
            .timed(
                "recon_reopen",
                sqlx::query(
                    r#"UPDATE reconciliation_state
                          SET status = 'pending', attempts = 0, last_error = NULL,
                              next_attempt_at = now(), updated_at = now()
                        WHERE kind = $1 AND subject = $2 AND status = 'resolved'"#,
                )
                .bind(kind)
                .bind(subject)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// True while a claim is actively unresolved (pending/in_progress).
    /// Position reconciliation uses this to detect in-flight executions
    /// before judging a balance divergence.
    pub async fn is_active(&self, kind: &str, subject: &str) -> RepoResult<bool> {
        let row: Option<(i64,)> = self
            .db
            .timed(
                "recon_is_active",
                sqlx::query_as(
                    r#"SELECT 1::bigint FROM reconciliation_state
                        WHERE kind = $1 AND subject = $2
                          AND status IN ('pending', 'in_progress')"#,
                )
                .bind(kind)
                .bind(subject)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.is_some())
    }

    pub async fn enqueue(&self, kind: &str, subject: &str) -> RepoResult<()> {
        self.db
            .timed(
                "recon_enqueue",
                sqlx::query(
                    r#"INSERT INTO reconciliation_state (subject, kind)
                       VALUES ($1, $2)
                       ON CONFLICT (kind, subject) DO UPDATE SET
                         status = CASE WHEN reconciliation_state.status = 'resolved'
                                       THEN 'resolved' ELSE 'pending' END,
                         next_attempt_at = LEAST(reconciliation_state.next_attempt_at, now())"#,
                )
                .bind(subject)
                .bind(kind)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Atomically claim up to `limit` due items (SKIP LOCKED keeps multiple
    /// workers safe).
    pub async fn claim_due(&self, limit: i64) -> RepoResult<Vec<ReconItem>> {
        let rows = self
            .db
            .timed(
                "recon_claim",
                sqlx::query(
                    r#"UPDATE reconciliation_state SET
                          status = 'in_progress', attempts = attempts + 1, updated_at = now(),
                          next_attempt_at = now() + interval '60 seconds'
                       WHERE (kind, subject) IN (
                           SELECT kind, subject FROM reconciliation_state
                            WHERE status IN ('pending','in_progress')
                              AND next_attempt_at <= now()
                              AND attempts < max_attempts
                            ORDER BY next_attempt_at ASC
                            LIMIT $1
                            FOR UPDATE SKIP LOCKED)
                       RETURNING kind, subject, attempts, max_attempts"#,
                )
                .bind(limit.clamp(1, 200))
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| ReconItem {
                kind: r.try_get("kind").unwrap_or_default(),
                subject: r.try_get("subject").unwrap_or_default(),
                attempts: r.try_get("attempts").unwrap_or(0),
                max_attempts: r.try_get("max_attempts").unwrap_or(10),
            })
            .collect())
    }

    pub async fn resolve(&self, kind: &str, subject: &str) -> RepoResult<()> {
        self.db
            .timed(
                "recon_resolve",
                sqlx::query(
                    r#"UPDATE reconciliation_state
                          SET status = 'resolved', last_error = NULL, updated_at = now()
                        WHERE kind = $1 AND subject = $2"#,
                )
                .bind(kind)
                .bind(subject)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Failure with exponential backoff (30s * 2^attempts, capped at 1h).
    /// Exhausted items land in `failed` for operator attention.
    pub async fn fail(&self, kind: &str, subject: &str, err: &str) -> RepoResult<()> {
        self.db
            .timed(
                "recon_fail",
                sqlx::query(
                    r#"UPDATE reconciliation_state SET
                          last_error = $3,
                          status = CASE WHEN attempts >= max_attempts THEN 'failed'
                                        ELSE 'pending' END,
                          next_attempt_at = now() + make_interval(
                              secs => LEAST(30 * POWER(2, attempts), 3600)::bigint),
                          updated_at = now()
                        WHERE kind = $1 AND subject = $2"#,
                )
                .bind(kind)
                .bind(subject)
                .bind(err.chars().take(500).collect::<String>())
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Park a subject permanently (operator queue) without burning the
    /// remaining attempts one by one.
    pub async fn give_up(&self, kind: &str, subject: &str, reason: &str) -> RepoResult<()> {
        self.db
            .timed(
                "recon_give_up",
                sqlx::query(
                    r#"UPDATE reconciliation_state SET
                          status = 'failed', last_error = $3,
                          attempts = max_attempts, updated_at = now()
                        WHERE kind = $1 AND subject = $2"#,
                )
                .bind(kind)
                .bind(subject)
                .bind(reason.chars().take(500).collect::<String>())
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Return an unprocessed claim to `pending` without consuming an
    /// attempt (used when the worker stops mid-batch).
    pub async fn release(&self, kind: &str, subject: &str) -> RepoResult<()> {
        self.db
            .timed(
                "recon_release",
                sqlx::query(
                    r#"UPDATE reconciliation_state SET
                          status = 'pending',
                          attempts = GREATEST(attempts - 1, 0),
                          next_attempt_at = now(), updated_at = now()
                        WHERE kind = $1 AND subject = $2 AND status = 'in_progress'"#,
                )
                .bind(kind)
                .bind(subject)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Items permanently stuck in `failed` (operator queue).
    pub async fn list_failed(&self, limit: i64) -> RepoResult<Vec<serde_json::Value>> {
        let rows = self
            .db
            .timed(
                "recon_failed",
                sqlx::query(
                    "SELECT * FROM reconciliation_state WHERE status = 'failed' ORDER BY updated_at DESC LIMIT $1",
                )
                .bind(limit.clamp(1, 500))
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "kind": r.try_get::<String, _>("kind").unwrap_or_default(),
                    "subject": r.try_get::<String, _>("subject").unwrap_or_default(),
                    "attempts": r.try_get::<i32, _>("attempts").unwrap_or(0),
                    "last_error": r.try_get::<Option<String>, _>("last_error").ok().flatten(),
                    "updated_at": ts(r, "updated_at").map(|t| t.to_rfc3339()).unwrap_or_default(),
                })
            })
            .collect())
    }
}

pub struct CheckpointRepo {
    db: Arc<Database>,
}

impl CheckpointRepo {
    pub fn new(db: Arc<Database>) -> Self {
        CheckpointRepo { db }
    }

    pub async fn save(&self, worker: &str, position: &serde_json::Value) -> RepoResult<()> {
        self.db
            .timed(
                "checkpoint_save",
                sqlx::query(
                    r#"INSERT INTO recovery_checkpoints (worker, position, updated_at)
                       VALUES ($1,$2,now())
                       ON CONFLICT (worker) DO UPDATE SET
                         position = EXCLUDED.position, updated_at = now()"#,
                )
                .bind(worker)
                .bind(position)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    pub async fn load(&self, worker: &str) -> RepoResult<Option<serde_json::Value>> {
        let row = self
            .db
            .timed(
                "checkpoint_load",
                sqlx::query("SELECT position FROM recovery_checkpoints WHERE worker = $1")
                    .bind(worker)
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.and_then(|r| r.try_get("position").ok()))
    }
}

// ---------------------------------------------------------------------------
// Balance snapshots
// ---------------------------------------------------------------------------

pub struct BalanceRepo {
    db: Arc<Database>,
}

impl BalanceRepo {
    pub fn new(db: Arc<Database>) -> Self {
        BalanceRepo { db }
    }

    pub async fn append(
        &self,
        chain: &str,
        address: &str,
        asset: &str,
        amount: f64,
        usd_value: Option<f64>,
        source: &str,
    ) -> RepoResult<()> {
        self.db
            .timed(
                "balance_append",
                sqlx::query(
                    r#"INSERT INTO balance_snapshots (chain, address, asset, amount, usd_value, source)
                       VALUES ($1,$2,$3,$4,$5,$6)"#,
                )
                .bind(chain)
                .bind(address)
                .bind(asset)
                .bind(amount)
                .bind(usd_value)
                .bind(source)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Latest snapshot per (address, asset) — reconciler expectation input.
    pub async fn latest_per_asset(&self) -> RepoResult<Vec<(String, String, f64)>> {
        let rows = self
            .db
            .timed(
                "balance_latest",
                sqlx::query(
                    r#"SELECT DISTINCT ON (address, asset) address, asset, amount
                         FROM balance_snapshots
                        ORDER BY address, asset, ts DESC"#,
                )
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    r.try_get::<String, _>("address").unwrap_or_default(),
                    r.try_get::<String, _>("asset").unwrap_or_default(),
                    r.try_get::<f64, _>("amount").unwrap_or(0.0),
                )
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chain hash is deterministic and sensitive to every field.
    #[test]
    fn audit_chain_hash_is_deterministic_and_field_sensitive() {
        let ts = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let detail = serde_json::json!({"k": 1});
        let h1 = AuditRepo::chain_hash(
            "prev",
            &ts,
            "actor",
            "action",
            Some("t"),
            "success",
            &detail,
        );
        let h2 = AuditRepo::chain_hash(
            "prev",
            &ts,
            "actor",
            "action",
            Some("t"),
            "success",
            &detail,
        );
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64, "sha256 hex");
        // Every field changes the hash.
        assert_ne!(
            h1,
            AuditRepo::chain_hash(
                "PREV",
                &ts,
                "actor",
                "action",
                Some("t"),
                "success",
                &detail
            )
        );
        assert_ne!(
            h1,
            AuditRepo::chain_hash(
                "prev",
                &ts,
                "actor2",
                "action",
                Some("t"),
                "success",
                &detail
            )
        );
        assert_ne!(
            h1,
            AuditRepo::chain_hash(
                "prev",
                &ts,
                "actor",
                "action2",
                Some("t"),
                "success",
                &detail
            )
        );
        assert_ne!(
            h1,
            AuditRepo::chain_hash("prev", &ts, "actor", "action", None, "success", &detail)
        );
        assert_ne!(
            h1,
            AuditRepo::chain_hash(
                "prev",
                &ts,
                "actor",
                "action",
                Some("t"),
                "failure",
                &detail
            )
        );
        assert_ne!(
            h1,
            AuditRepo::chain_hash(
                "prev",
                &ts,
                "actor",
                "action",
                Some("t"),
                "success",
                &serde_json::json!({"k": 2})
            )
        );
        let ts2 = DateTime::from_timestamp(1_700_000_001, 0).unwrap();
        assert_ne!(
            h1,
            AuditRepo::chain_hash(
                "prev",
                &ts2,
                "actor",
                "action",
                Some("t"),
                "success",
                &detail
            )
        );
    }

    /// Order row mapping tolerates garbage columns (defensive defaults) so a
    /// partially-written row can never panic the recovery path.
    #[test]
    fn order_status_parsing_defaults_to_unknown() {
        assert_eq!(OrderStatus::parse("bogus"), None);
        assert!(OrderStatus::parse("filled").unwrap().is_terminal());
        assert!(!OrderStatus::parse("submitted").unwrap().is_terminal());
    }
}
