//! Durable copy-trading repository (migration 0013, TASK 3).
//!
//! Backs Module 2's leader registry, processed-event journal and
//! leader ↔ follower links with PostgreSQL. Same conventions as `repo.rs`
//! and `execution.rs`: runtime-checked queries, upserts / guarded inserts,
//! every call through [`Database::timed`]. The record types are plain data
//! so `module-copy` can use them without depending on `sqlx`.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::db::{Database, TimedDbError};

type RepoResult<T> = Result<T, TimedDbError>;

/// One tracked leader (wallet) and its lifecycle state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaderRecord {
    pub address: String,
    pub label: String,
    /// `active` | `paused` | `removed`.
    pub status: String,
    /// Where the leader came from (`config` today).
    pub source: String,
    pub followed_at: DateTime<Utc>,
    pub status_since: DateTime<Utc>,
    pub events_seen: i64,
    pub mirrored: i64,
    pub rejected: i64,
    pub last_event_at: Option<DateTime<Utc>>,
    pub last_slot: Option<i64>,
    pub updated_at: DateTime<Utc>,
}

/// One append-only leader lifecycle transition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaderEventRecord {
    #[serde(default)]
    pub id: i64,
    pub address: String,
    /// `followed` | `paused` | `resumed` | `unfollowed` | `rule_changed`.
    pub event: String,
    pub reason: Option<String>,
    pub replica_id: String,
    pub ts: DateTime<Utc>,
}

/// One leader-trade event the pipeline finished with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CopyEventRecord {
    pub event_id: String,
    pub leader: String,
    pub signature: String,
    pub slot: u64,
    pub mint: String,
    /// `buy` | `sell`.
    pub side: String,
    pub venue: String,
    pub token_amount: f64,
    pub sol_amount: f64,
    pub source: String,
    pub source_sequence: u64,
    pub event_at: Option<DateTime<Utc>>,
    pub observed_at: DateTime<Utc>,
    /// Final pipeline stage (`CONFIRMED`, `REJECTED`, …).
    pub stage: String,
    pub reject_reason: Option<String>,
    pub detail: Option<String>,
    pub intent_id: Option<String>,
    pub position_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Follower position ↔ the leader entry it mirrors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CopyLinkRecord {
    pub position_id: String,
    pub leader: String,
    pub mint: String,
    pub entry_event_id: String,
    pub entry_signature: String,
    pub intent_id: Option<String>,
    /// Tokens the leader bought in the mirrored trade (their size, not ours).
    pub leader_token_amount: f64,
    pub follower_qty: f64,
    /// `open` | `closed` | `orphaned` | `mismatch`.
    pub status: String,
    pub opened_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub exit_event_id: Option<String>,
    pub last_reconciled_at: Option<DateTime<Utc>>,
    pub note: Option<String>,
    pub updated_at: DateTime<Utc>,
}

pub struct CopyRepo {
    db: Arc<Database>,
}

impl CopyRepo {
    pub fn new(db: Arc<Database>) -> Self {
        CopyRepo { db }
    }

    // ------------------------------------------------------------ leaders --

    /// Upsert a leader. `followed_at` keeps the first-seen time; status,
    /// counters and `status_since` follow the record.
    pub async fn upsert_leader(&self, rec: &LeaderRecord) -> RepoResult<()> {
        self.db
            .timed(
                "copy_leader_upsert",
                sqlx::query(
                    r#"INSERT INTO copy_leaders
                           (address, label, status, source, followed_at, status_since,
                            events_seen, mirrored, rejected, last_event_at, last_slot, updated_at)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                       ON CONFLICT (address) DO UPDATE SET
                           label = EXCLUDED.label,
                           status = EXCLUDED.status,
                           source = EXCLUDED.source,
                           status_since = EXCLUDED.status_since,
                           events_seen = GREATEST(copy_leaders.events_seen, EXCLUDED.events_seen),
                           mirrored = GREATEST(copy_leaders.mirrored, EXCLUDED.mirrored),
                           rejected = GREATEST(copy_leaders.rejected, EXCLUDED.rejected),
                           last_event_at = COALESCE(EXCLUDED.last_event_at, copy_leaders.last_event_at),
                           last_slot = GREATEST(copy_leaders.last_slot, EXCLUDED.last_slot),
                           updated_at = EXCLUDED.updated_at"#,
                )
                .bind(&rec.address)
                .bind(&rec.label)
                .bind(&rec.status)
                .bind(&rec.source)
                .bind(rec.followed_at)
                .bind(rec.status_since)
                .bind(rec.events_seen)
                .bind(rec.mirrored)
                .bind(rec.rejected)
                .bind(rec.last_event_at)
                .bind(rec.last_slot)
                .bind(rec.updated_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    pub async fn get_leader(&self, address: &str) -> RepoResult<Option<LeaderRecord>> {
        let row = self
            .db
            .timed(
                "copy_leader_get",
                sqlx::query("SELECT * FROM copy_leaders WHERE address = $1")
                    .bind(address)
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.as_ref().map(leader_from_row))
    }

    /// Every leader row (including removed ones — the caller decides).
    pub async fn list_leaders(&self) -> RepoResult<Vec<LeaderRecord>> {
        let rows = self
            .db
            .timed(
                "copy_leader_list",
                sqlx::query("SELECT * FROM copy_leaders ORDER BY followed_at ASC, address ASC")
                    .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(leader_from_row).collect())
    }

    pub async fn append_leader_event(&self, rec: &LeaderEventRecord) -> RepoResult<()> {
        self.db
            .timed(
                "copy_leader_event",
                sqlx::query(
                    r#"INSERT INTO copy_leader_events (address, event, reason, replica_id, ts)
                       VALUES ($1, $2, $3, $4, $5)"#,
                )
                .bind(&rec.address)
                .bind(&rec.event)
                .bind(&rec.reason)
                .bind(&rec.replica_id)
                .bind(rec.ts)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Lifecycle history of one leader, oldest first.
    pub async fn leader_events(
        &self,
        address: &str,
        limit: i64,
    ) -> RepoResult<Vec<LeaderEventRecord>> {
        let rows = self
            .db
            .timed(
                "copy_leader_events",
                sqlx::query(
                    r#"SELECT * FROM copy_leader_events
                        WHERE address = $1 ORDER BY ts ASC, id ASC LIMIT $2"#,
                )
                .bind(address)
                .bind(limit.clamp(1, 1000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(leader_event_from_row).collect())
    }

    // ------------------------------------------------------------- events --

    /// Upsert the final outcome of one leader-trade event. Idempotent: the
    /// same snapshot is a no-op; a later outcome overwrites stage/reason/
    /// intent/position while `created_at` keeps the first-seen time.
    pub async fn record_event(&self, rec: &CopyEventRecord) -> RepoResult<()> {
        self.db
            .timed(
                "copy_event_record",
                sqlx::query(
                    r#"INSERT INTO copy_events
                           (event_id, leader, signature, slot, mint, side, venue,
                            token_amount, sol_amount, source, source_sequence,
                            event_at, observed_at, stage, reject_reason, detail,
                            intent_id, position_id, created_at, updated_at)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                               $12, $13, $14, $15, $16, $17, $18, $19, $20)
                       ON CONFLICT (event_id) DO UPDATE SET
                           stage = EXCLUDED.stage,
                           reject_reason = EXCLUDED.reject_reason,
                           detail = EXCLUDED.detail,
                           intent_id = COALESCE(EXCLUDED.intent_id, copy_events.intent_id),
                           position_id = COALESCE(EXCLUDED.position_id, copy_events.position_id),
                           updated_at = EXCLUDED.updated_at"#,
                )
                .bind(&rec.event_id)
                .bind(&rec.leader)
                .bind(&rec.signature)
                .bind(rec.slot as i64)
                .bind(&rec.mint)
                .bind(&rec.side)
                .bind(&rec.venue)
                .bind(rec.token_amount)
                .bind(rec.sol_amount)
                .bind(&rec.source)
                .bind(rec.source_sequence as i64)
                .bind(rec.event_at)
                .bind(rec.observed_at)
                .bind(&rec.stage)
                .bind(&rec.reject_reason)
                .bind(&rec.detail)
                .bind(&rec.intent_id)
                .bind(&rec.position_id)
                .bind(rec.created_at)
                .bind(rec.updated_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    pub async fn get_event(&self, event_id: &str) -> RepoResult<Option<CopyEventRecord>> {
        let row = self
            .db
            .timed(
                "copy_event_get",
                sqlx::query("SELECT * FROM copy_events WHERE event_id = $1")
                    .bind(event_id)
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.as_ref().map(event_from_row))
    }

    /// Events observed at or after `since`, oldest first — the restart
    /// recovery re-seeds the dedup facade from these.
    pub async fn events_since(
        &self,
        since: DateTime<Utc>,
        limit: i64,
    ) -> RepoResult<Vec<CopyEventRecord>> {
        let rows = self
            .db
            .timed(
                "copy_events_since",
                sqlx::query(
                    r#"SELECT * FROM copy_events
                        WHERE observed_at >= $1 ORDER BY observed_at ASC LIMIT $2"#,
                )
                .bind(since)
                .bind(limit.clamp(1, 10_000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(event_from_row).collect())
    }

    /// Most recent events of one leader, newest first.
    pub async fn events_for_leader(
        &self,
        leader: &str,
        limit: i64,
    ) -> RepoResult<Vec<CopyEventRecord>> {
        let rows = self
            .db
            .timed(
                "copy_events_leader",
                sqlx::query(
                    r#"SELECT * FROM copy_events
                        WHERE leader = $1 ORDER BY observed_at DESC LIMIT $2"#,
                )
                .bind(leader)
                .bind(limit.clamp(1, 1000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(event_from_row).collect())
    }

    // -------------------------------------------------------------- links --

    /// Upsert a link. `opened_at` keeps the first value; status, quantities
    /// and reconciliation marks follow the record.
    pub async fn upsert_link(&self, rec: &CopyLinkRecord) -> RepoResult<()> {
        self.db
            .timed(
                "copy_link_upsert",
                sqlx::query(
                    r#"INSERT INTO copy_links
                           (position_id, leader, mint, entry_event_id, entry_signature, intent_id,
                            leader_token_amount, follower_qty, status, opened_at, closed_at,
                            exit_event_id, last_reconciled_at, note, updated_at)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
                       ON CONFLICT (position_id) DO UPDATE SET
                           intent_id = COALESCE(EXCLUDED.intent_id, copy_links.intent_id),
                           leader_token_amount = EXCLUDED.leader_token_amount,
                           follower_qty = EXCLUDED.follower_qty,
                           status = EXCLUDED.status,
                           closed_at = EXCLUDED.closed_at,
                           exit_event_id = COALESCE(EXCLUDED.exit_event_id, copy_links.exit_event_id),
                           last_reconciled_at = EXCLUDED.last_reconciled_at,
                           note = EXCLUDED.note,
                           updated_at = EXCLUDED.updated_at"#,
                )
                .bind(&rec.position_id)
                .bind(&rec.leader)
                .bind(&rec.mint)
                .bind(&rec.entry_event_id)
                .bind(&rec.entry_signature)
                .bind(&rec.intent_id)
                .bind(rec.leader_token_amount)
                .bind(rec.follower_qty)
                .bind(&rec.status)
                .bind(rec.opened_at)
                .bind(rec.closed_at)
                .bind(&rec.exit_event_id)
                .bind(rec.last_reconciled_at)
                .bind(&rec.note)
                .bind(rec.updated_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    pub async fn get_link(&self, position_id: &str) -> RepoResult<Option<CopyLinkRecord>> {
        let row = self
            .db
            .timed(
                "copy_link_get",
                sqlx::query("SELECT * FROM copy_links WHERE position_id = $1")
                    .bind(position_id)
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.as_ref().map(link_from_row))
    }

    /// Links that are still open — the reconciliation input.
    pub async fn open_links(&self) -> RepoResult<Vec<CopyLinkRecord>> {
        let rows = self
            .db
            .timed(
                "copy_links_open",
                sqlx::query(
                    "SELECT * FROM copy_links WHERE status = 'open' ORDER BY opened_at ASC LIMIT 5000",
                )
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().map(link_from_row).collect())
    }

    /// Mark a link closed / orphaned / mismatch. Only open links transition.
    pub async fn close_link(
        &self,
        position_id: &str,
        status: &str,
        exit_event_id: Option<&str>,
        note: Option<&str>,
    ) -> RepoResult<bool> {
        let res = self
            .db
            .timed(
                "copy_link_close",
                sqlx::query(
                    r#"UPDATE copy_links
                          SET status = $2, exit_event_id = COALESCE($3, exit_event_id),
                              note = COALESCE($4, note), closed_at = now(), updated_at = now()
                        WHERE position_id = $1 AND status = 'open'"#,
                )
                .bind(position_id)
                .bind(status)
                .bind(exit_event_id)
                .bind(note)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected() > 0)
    }
}

fn leader_from_row(r: &PgRow) -> LeaderRecord {
    LeaderRecord {
        address: r.try_get("address").unwrap_or_default(),
        label: r.try_get("label").unwrap_or_default(),
        status: r.try_get("status").unwrap_or_default(),
        source: r.try_get("source").unwrap_or_default(),
        followed_at: r.try_get("followed_at").unwrap_or_else(|_| Utc::now()),
        status_since: r.try_get("status_since").unwrap_or_else(|_| Utc::now()),
        events_seen: r.try_get("events_seen").unwrap_or_default(),
        mirrored: r.try_get("mirrored").unwrap_or_default(),
        rejected: r.try_get("rejected").unwrap_or_default(),
        last_event_at: r.try_get("last_event_at").unwrap_or_default(),
        last_slot: r.try_get("last_slot").unwrap_or_default(),
        updated_at: r.try_get("updated_at").unwrap_or_else(|_| Utc::now()),
    }
}

fn leader_event_from_row(r: &PgRow) -> LeaderEventRecord {
    LeaderEventRecord {
        id: r.try_get("id").unwrap_or_default(),
        address: r.try_get("address").unwrap_or_default(),
        event: r.try_get("event").unwrap_or_default(),
        reason: r.try_get("reason").unwrap_or_default(),
        replica_id: r.try_get("replica_id").unwrap_or_default(),
        ts: r.try_get("ts").unwrap_or_else(|_| Utc::now()),
    }
}

fn event_from_row(r: &PgRow) -> CopyEventRecord {
    CopyEventRecord {
        event_id: r.try_get("event_id").unwrap_or_default(),
        leader: r.try_get("leader").unwrap_or_default(),
        signature: r.try_get("signature").unwrap_or_default(),
        slot: r.try_get::<i64, _>("slot").unwrap_or_default().max(0) as u64,
        mint: r.try_get("mint").unwrap_or_default(),
        side: r.try_get("side").unwrap_or_default(),
        venue: r.try_get("venue").unwrap_or_default(),
        token_amount: r.try_get("token_amount").unwrap_or_default(),
        sol_amount: r.try_get("sol_amount").unwrap_or_default(),
        source: r.try_get("source").unwrap_or_default(),
        source_sequence: r
            .try_get::<i64, _>("source_sequence")
            .unwrap_or_default()
            .max(0) as u64,
        event_at: r.try_get("event_at").unwrap_or_default(),
        observed_at: r.try_get("observed_at").unwrap_or_else(|_| Utc::now()),
        stage: r.try_get("stage").unwrap_or_default(),
        reject_reason: r.try_get("reject_reason").unwrap_or_default(),
        detail: r.try_get("detail").unwrap_or_default(),
        intent_id: r.try_get("intent_id").unwrap_or_default(),
        position_id: r.try_get("position_id").unwrap_or_default(),
        created_at: r.try_get("created_at").unwrap_or_else(|_| Utc::now()),
        updated_at: r.try_get("updated_at").unwrap_or_else(|_| Utc::now()),
    }
}

fn link_from_row(r: &PgRow) -> CopyLinkRecord {
    CopyLinkRecord {
        position_id: r.try_get("position_id").unwrap_or_default(),
        leader: r.try_get("leader").unwrap_or_default(),
        mint: r.try_get("mint").unwrap_or_default(),
        entry_event_id: r.try_get("entry_event_id").unwrap_or_default(),
        entry_signature: r.try_get("entry_signature").unwrap_or_default(),
        intent_id: r.try_get("intent_id").unwrap_or_default(),
        leader_token_amount: r.try_get("leader_token_amount").unwrap_or_default(),
        follower_qty: r.try_get("follower_qty").unwrap_or_default(),
        status: r.try_get("status").unwrap_or_default(),
        opened_at: r.try_get("opened_at").unwrap_or_else(|_| Utc::now()),
        closed_at: r.try_get("closed_at").unwrap_or_default(),
        exit_event_id: r.try_get("exit_event_id").unwrap_or_default(),
        last_reconciled_at: r.try_get("last_reconciled_at").unwrap_or_default(),
        note: r.try_get("note").unwrap_or_default(),
        updated_at: r.try_get("updated_at").unwrap_or_else(|_| Utc::now()),
    }
}
