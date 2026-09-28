//! Durable execution-lifecycle repository (migration 0012, TASK 1).
//!
//! Backs [`crate::execution::ExecutionLedger`] with PostgreSQL so a crash at
//! any point of the transaction lifecycle leaves enough state to recover:
//! one upserted row per intent (`execution_lifecycle`) plus an append-only
//! transition history (`execution_lifecycle_events`). Same conventions as
//! `repo.rs`: runtime-checked queries, upserts/guarded inserts, every call
//! through [`Database::timed`].

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::db::{Database, TimedDbError};
use crate::execution::{ExecutionRecord, ExecutionState, ExecutionTransition, FailureClass};

type RepoResult<T> = Result<T, TimedDbError>;

pub struct ExecutionRepo {
    db: Arc<Database>,
}

impl ExecutionRepo {
    pub fn new(db: Arc<Database>) -> Self {
        ExecutionRepo { db }
    }

    /// Upsert the current state of one intent. Idempotent: re-writing the
    /// same snapshot is a no-op; a newer snapshot overwrites state, attempt
    /// and outcome columns while `created_at` keeps the first-seen time.
    pub async fn upsert(&self, rec: &ExecutionRecord) -> RepoResult<()> {
        self.db
            .timed(
                "execution_upsert",
                sqlx::query(
                    r#"INSERT INTO execution_lifecycle
                           (intent_id, module, label, wallet, symbol, state, attempts,
                            signature, blockhash, last_valid_block_height,
                            priority_fee_micro_lamports, failure_class, error,
                            created_at, updated_at)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
                       ON CONFLICT (intent_id) DO UPDATE SET
                           state = EXCLUDED.state,
                           attempts = GREATEST(execution_lifecycle.attempts, EXCLUDED.attempts),
                           signature = EXCLUDED.signature,
                           blockhash = EXCLUDED.blockhash,
                           last_valid_block_height = EXCLUDED.last_valid_block_height,
                           priority_fee_micro_lamports = EXCLUDED.priority_fee_micro_lamports,
                           failure_class = EXCLUDED.failure_class,
                           error = EXCLUDED.error,
                           updated_at = EXCLUDED.updated_at"#,
                )
                .bind(&rec.intent_id)
                .bind(&rec.module)
                .bind(&rec.label)
                .bind(&rec.wallet)
                .bind(&rec.symbol)
                .bind(rec.state.as_str())
                .bind(rec.attempts as i32)
                .bind(&rec.signature)
                .bind(&rec.blockhash)
                .bind(rec.last_valid_block_height.map(|h| h as i64))
                .bind(rec.priority_fee_micro_lamports as i64)
                .bind(rec.failure.map(|f| f.as_str()))
                .bind(&rec.error)
                .bind(rec.created_at)
                .bind(rec.updated_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Append one transition to the immutable history.
    pub async fn append_event(&self, t: &ExecutionTransition) -> RepoResult<()> {
        self.db
            .timed(
                "execution_event",
                sqlx::query(
                    r#"INSERT INTO execution_lifecycle_events
                           (intent_id, attempt, from_state, to_state, signature,
                            failure_class, reason, ts)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"#,
                )
                .bind(&t.intent_id)
                .bind(t.attempt as i32)
                .bind(t.from.map(|s| s.as_str()))
                .bind(t.to.as_str())
                .bind(&t.signature)
                .bind(t.failure.map(|f| f.as_str()))
                .bind(&t.reason)
                .bind(t.ts)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    pub async fn get(&self, intent_id: &str) -> RepoResult<Option<ExecutionRecord>> {
        let row = self
            .db
            .timed(
                "execution_get",
                sqlx::query("SELECT * FROM execution_lifecycle WHERE intent_id = $1")
                    .bind(intent_id)
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.as_ref().map(record_from_row))
    }

    pub async fn get_by_signature(&self, signature: &str) -> RepoResult<Option<ExecutionRecord>> {
        let row = self
            .db
            .timed(
                "execution_get_by_sig",
                sqlx::query(
                    "SELECT * FROM execution_lifecycle WHERE signature = $1 \
                     ORDER BY updated_at DESC LIMIT 1",
                )
                .bind(signature)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.as_ref().map(record_from_row))
    }

    /// Attempts that are not settled — the crash-recovery input.
    pub async fn list_open(&self) -> RepoResult<Vec<ExecutionRecord>> {
        let rows = self
            .db
            .timed(
                "execution_open",
                sqlx::query(
                    r#"SELECT * FROM execution_lifecycle
                        WHERE state IN ('created', 'validated', 'submitted', 'pending')
                        ORDER BY updated_at ASC LIMIT 5000"#,
                )
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(record_from_row).collect())
    }

    pub async fn list_recent(&self, limit: i64) -> RepoResult<Vec<ExecutionRecord>> {
        let rows = self
            .db
            .timed(
                "execution_recent",
                sqlx::query("SELECT * FROM execution_lifecycle ORDER BY updated_at DESC LIMIT $1")
                    .bind(limit.clamp(1, 1000))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(record_from_row).collect())
    }

    /// Transition history for one intent, oldest first.
    pub async fn events(&self, intent_id: &str, limit: i64) -> RepoResult<Vec<serde_json::Value>> {
        let rows = self
            .db
            .timed(
                "execution_events",
                sqlx::query(
                    r#"SELECT id, intent_id, attempt, from_state, to_state, signature,
                              failure_class, reason, ts
                         FROM execution_lifecycle_events
                        WHERE intent_id = $1 ORDER BY ts ASC, id ASC LIMIT $2"#,
                )
                .bind(intent_id)
                .bind(limit.clamp(1, 1000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.try_get::<i64, _>("id").unwrap_or_default(),
                    "intent_id": r.try_get::<String, _>("intent_id").unwrap_or_default(),
                    "attempt": r.try_get::<i32, _>("attempt").unwrap_or(1),
                    "from": r.try_get::<Option<String>, _>("from_state").unwrap_or_default(),
                    "to": r.try_get::<String, _>("to_state").unwrap_or_default(),
                    "signature": r.try_get::<Option<String>, _>("signature").unwrap_or_default(),
                    "failure_class": r.try_get::<Option<String>, _>("failure_class").unwrap_or_default(),
                    "reason": r.try_get::<Option<String>, _>("reason").unwrap_or_default(),
                    "ts": r.try_get::<DateTime<Utc>, _>("ts").ok(),
                })
            })
            .collect())
    }

    /// Delete settled rows older than `age` (maintenance; open rows are
    /// never touched). Returns the number of rows removed.
    pub async fn delete_settled_older_than(&self, age: chrono::Duration) -> RepoResult<u64> {
        let cutoff = Utc::now() - age;
        let res = self
            .db
            .timed(
                "execution_prune",
                sqlx::query(
                    r#"DELETE FROM execution_lifecycle
                        WHERE state IN ('confirmed', 'failed', 'expired', 'reconciled')
                          AND updated_at < $1"#,
                )
                .bind(cutoff)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected())
    }
}

fn record_from_row(row: &PgRow) -> ExecutionRecord {
    let now = Utc::now();
    ExecutionRecord {
        intent_id: row.try_get("intent_id").unwrap_or_default(),
        module: row.try_get("module").unwrap_or_default(),
        label: row.try_get("label").unwrap_or_default(),
        wallet: row.try_get("wallet").unwrap_or_default(),
        symbol: row.try_get("symbol").unwrap_or_default(),
        state: row
            .try_get::<String, _>("state")
            .ok()
            .and_then(|s| ExecutionState::parse(&s))
            .unwrap_or(ExecutionState::Pending),
        attempts: row.try_get::<i32, _>("attempts").unwrap_or(1).max(1) as u32,
        signature: row.try_get("signature").unwrap_or_default(),
        blockhash: row.try_get("blockhash").unwrap_or_default(),
        last_valid_block_height: row
            .try_get::<Option<i64>, _>("last_valid_block_height")
            .unwrap_or_default()
            .map(|h| h.max(0) as u64),
        priority_fee_micro_lamports: row
            .try_get::<i64, _>("priority_fee_micro_lamports")
            .unwrap_or_default()
            .max(0) as u64,
        failure: row
            .try_get::<Option<String>, _>("failure_class")
            .unwrap_or_default()
            .and_then(|s| FailureClass::parse(&s)),
        error: row.try_get("error").unwrap_or_default(),
        created_at: row.try_get("created_at").unwrap_or(now),
        updated_at: row.try_get("updated_at").unwrap_or(now),
        entered_at: std::time::Instant::now(),
    }
}
