//! Durable HA repository (migration 0016, TASK 6).
//!
//! Backs the worker registry, the singleton role leases (with fencing
//! generations), the durable feed cursors, the detected feed gaps and the
//! recovery journal with PostgreSQL. Same conventions as `copy.rs` /
//! `polymarket.rs` / `accounting.rs`: runtime-checked queries, guarded
//! upserts, every call through [`Database::timed`]; the record types are
//! the plain data types of [`crate::ha`], so no caller needs `sqlx`.
//!
//! The safety-critical statement is [`HaRepo::acquire_lease`]: ONE
//! `INSERT … ON CONFLICT DO UPDATE … WHERE` decides the winner inside the
//! database, so two workers racing for the same role can never both get it.
//! The `WHERE` clause admits exactly three situations — the lease expired,
//! it was released, or the caller already holds it — and every successful
//! path bumps the fencing `generation`.

use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::db::{Database, TimedDbError};
use crate::ha::cursor::{FeedCursor, FeedGap, FeedId, GapStatus};
use crate::ha::lease::{Lease, LeaseDecision, LeaseRequest, LeaseRole};
use crate::ha::recovery_plan::OrderRecoveryAction;
use crate::ha::store::RecoveryRecord;
use crate::ha::worker::{HaMode, WorkerRegistration, WorkerState};

type RepoResult<T> = Result<T, TimedDbError>;

/// Repository over the 0016 tables.
pub struct HaRepo {
    db: Arc<Database>,
}

impl HaRepo {
    /// Repository over `db`.
    pub fn new(db: Arc<Database>) -> Self {
        HaRepo { db }
    }

    /// The shared database clock — the reference for every liveness
    /// comparison (worker clocks drift independently).
    pub async fn now(&self) -> RepoResult<DateTime<Utc>> {
        let row = self
            .db
            .timed(
                "ha_now",
                sqlx::query("SELECT now() AS t").fetch_one(self.db.pool()),
            )
            .await?;
        Ok(row.try_get("t").unwrap_or_else(|_| Utc::now()))
    }

    // ------------------------------------------------------------ workers --

    /// Register this worker life: keep the identity, take the next
    /// generation, reset the state to `starting`.
    pub async fn register_worker(
        &self,
        worker_id: &str,
        mode: &str,
        host: &str,
        pid: i64,
        version: &str,
    ) -> RepoResult<WorkerRegistration> {
        let row = self
            .db
            .timed(
                "ha_worker_register",
                sqlx::query(
                    r#"INSERT INTO ha_workers
                           (worker_id, generation, mode, state, host, pid, version, detail,
                            started_at, last_seen_at)
                       VALUES ($1, 1, $2, 'starting', $3, $4, $5, 'registered', now(), now())
                       ON CONFLICT (worker_id) DO UPDATE SET
                           generation = ha_workers.generation + 1,
                           mode = EXCLUDED.mode,
                           state = 'starting',
                           host = EXCLUDED.host,
                           pid = EXCLUDED.pid,
                           version = EXCLUDED.version,
                           detail = 'registered',
                           started_at = now(),
                           last_seen_at = now()
                       RETURNING worker_id, generation, mode, state, host, pid, version, detail,
                                 started_at, last_seen_at"#,
                )
                .bind(worker_id)
                .bind(mode)
                .bind(host)
                .bind(pid)
                .bind(version)
                .fetch_one(self.db.pool()),
            )
            .await?;
        let reg = worker_from_row(&row).ok_or_else(|| {
            TimedDbError::Error(sqlx::Error::Protocol("worker row unreadable".into()))
        })?;
        self.log_worker_event(
            worker_id,
            reg.generation,
            "registered",
            Some("starting"),
            "",
        )
        .await;
        Ok(reg)
    }

    /// Heartbeat + state for one generation. `false` = the row belongs to a
    /// newer generation (this life is stale).
    pub async fn heartbeat(
        &self,
        worker_id: &str,
        generation: i64,
        state: WorkerState,
        detail: &str,
    ) -> RepoResult<bool> {
        let res = self
            .db
            .timed(
                "ha_worker_heartbeat",
                sqlx::query(
                    r#"UPDATE ha_workers
                          SET state = $3, detail = $4, last_seen_at = now()
                        WHERE worker_id = $1 AND generation = $2"#,
                )
                .bind(worker_id)
                .bind(generation)
                .bind(state.as_str())
                .bind(detail)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Every registered worker.
    pub async fn workers(&self) -> RepoResult<Vec<WorkerRegistration>> {
        let rows = self
            .db
            .timed(
                "ha_workers_list",
                sqlx::query(
                    r#"SELECT worker_id, generation, mode, state, host, pid, version, detail,
                              started_at, last_seen_at
                         FROM ha_workers ORDER BY worker_id"#,
                )
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().filter_map(worker_from_row).collect())
    }

    /// Append one worker lifecycle event (best-effort; never fatal).
    pub async fn log_worker_event(
        &self,
        worker_id: &str,
        generation: i64,
        event: &str,
        state: Option<&str>,
        detail: &str,
    ) {
        let _ = self
            .db
            .timed(
                "ha_worker_event",
                sqlx::query(
                    r#"INSERT INTO ha_worker_events (worker_id, generation, event, state, detail)
                       VALUES ($1, $2, $3, $4, $5)"#,
                )
                .bind(worker_id)
                .bind(generation)
                .bind(event)
                .bind(state)
                .bind(detail)
                .execute(self.db.pool()),
            )
            .await;
    }

    // ------------------------------------------------------------- leases --

    /// Atomic acquisition. One statement decides the winner.
    pub async fn acquire_lease(&self, req: &LeaseRequest) -> RepoResult<LeaseDecision> {
        let role = req.role.as_string();
        let ttl_secs = req.ttl.num_seconds().max(1);
        let row = self
            .db
            .timed(
                "ha_lease_acquire",
                sqlx::query(
                    r#"INSERT INTO ha_leases
                           (role, holder, generation, acquired_at, renewed_at, expires_at,
                            takeover_count, previous_holder, released)
                       VALUES ($1, $2, 1, now(), now(), now() + make_interval(secs => $3), 0, NULL, false)
                       ON CONFLICT (role) DO UPDATE SET
                           holder = EXCLUDED.holder,
                           generation = ha_leases.generation + 1,
                           acquired_at = now(),
                           renewed_at = now(),
                           expires_at = now() + make_interval(secs => $3),
                           takeover_count = ha_leases.takeover_count
                               + CASE WHEN ha_leases.holder <> EXCLUDED.holder
                                       AND ha_leases.released = false
                                      THEN 1 ELSE 0 END,
                           previous_holder = ha_leases.holder,
                           released = false
                       WHERE ha_leases.expires_at <= now()
                          OR ha_leases.released = true
                          OR ha_leases.holder = EXCLUDED.holder
                       RETURNING role, holder, generation, acquired_at, renewed_at, expires_at,
                                 takeover_count, previous_holder, released"#,
                )
                .bind(&role)
                .bind(&req.holder)
                .bind(ttl_secs as f64)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        match row.as_ref().and_then(lease_from_row) {
            Some(lease) => {
                let event = if lease
                    .previous_holder
                    .as_deref()
                    .map(|p| p != lease.holder)
                    .unwrap_or(false)
                {
                    "takeover"
                } else {
                    "acquired"
                };
                self.log_lease_event(&role, &lease.holder, lease.generation, event, "")
                    .await;
                Ok(LeaseDecision::Acquired(lease))
            }
            None => {
                // The WHERE clause refused: somebody else holds a live lease.
                let current = self.get_lease(&req.role).await?;
                match current {
                    Some(l) => Ok(LeaseDecision::Rejected {
                        holder: l.holder,
                        generation: l.generation,
                        expires_at: l.expires_at,
                    }),
                    None => Err(TimedDbError::Error(sqlx::Error::Protocol(
                        "lease row vanished during acquisition".into(),
                    ))),
                }
            }
        }
    }

    /// Extend a lease; CAS on `(holder, generation)` and not expired.
    pub async fn renew_lease(
        &self,
        role: &LeaseRole,
        holder: &str,
        generation: i64,
        ttl: ChronoDuration,
    ) -> RepoResult<bool> {
        let res = self
            .db
            .timed(
                "ha_lease_renew",
                sqlx::query(
                    r#"UPDATE ha_leases
                          SET expires_at = now() + make_interval(secs => $4), renewed_at = now()
                        WHERE role = $1 AND holder = $2 AND generation = $3
                          AND released = false AND expires_at > now()"#,
                )
                .bind(role.as_string())
                .bind(holder)
                .bind(generation)
                .bind(ttl.num_seconds().max(1) as f64)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Fencing check: is `(holder, generation)` still the live owner?
    pub async fn verify_lease(
        &self,
        role: &LeaseRole,
        holder: &str,
        generation: i64,
    ) -> RepoResult<bool> {
        let row = self
            .db
            .timed(
                "ha_lease_verify",
                sqlx::query(
                    r#"SELECT 1 AS ok FROM ha_leases
                        WHERE role = $1 AND holder = $2 AND generation = $3
                          AND released = false AND expires_at > now()"#,
                )
                .bind(role.as_string())
                .bind(holder)
                .bind(generation)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.is_some())
    }

    /// Release a lease; CAS on `(holder, generation)`.
    pub async fn release_lease(
        &self,
        role: &LeaseRole,
        holder: &str,
        generation: i64,
    ) -> RepoResult<bool> {
        let res = self
            .db
            .timed(
                "ha_lease_release",
                sqlx::query(
                    r#"UPDATE ha_leases
                          SET released = true, expires_at = now()
                        WHERE role = $1 AND holder = $2 AND generation = $3 AND released = false"#,
                )
                .bind(role.as_string())
                .bind(holder)
                .bind(generation)
                .execute(self.db.pool()),
            )
            .await?;
        let ok = res.rows_affected() == 1;
        if ok {
            self.log_lease_event(&role.as_string(), holder, generation, "released", "")
                .await;
        }
        Ok(ok)
    }

    /// Current record for a role.
    pub async fn get_lease(&self, role: &LeaseRole) -> RepoResult<Option<Lease>> {
        let row = self
            .db
            .timed(
                "ha_lease_get",
                sqlx::query(
                    r#"SELECT role, holder, generation, acquired_at, renewed_at, expires_at,
                              takeover_count, previous_holder, released
                         FROM ha_leases WHERE role = $1"#,
                )
                .bind(role.as_string())
                .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.as_ref().and_then(lease_from_row))
    }

    /// Every lease.
    pub async fn leases(&self) -> RepoResult<Vec<Lease>> {
        let rows = self
            .db
            .timed(
                "ha_leases_list",
                sqlx::query(
                    r#"SELECT role, holder, generation, acquired_at, renewed_at, expires_at,
                              takeover_count, previous_holder, released
                         FROM ha_leases ORDER BY role"#,
                )
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().filter_map(lease_from_row).collect())
    }

    /// Append one lease lifecycle event (best-effort).
    pub async fn log_lease_event(
        &self,
        role: &str,
        holder: &str,
        generation: i64,
        event: &str,
        detail: &str,
    ) {
        let _ = self
            .db
            .timed(
                "ha_lease_event",
                sqlx::query(
                    r#"INSERT INTO ha_lease_events (role, holder, generation, event, detail)
                       VALUES ($1, $2, $3, $4, $5)"#,
                )
                .bind(role)
                .bind(holder)
                .bind(generation)
                .bind(event)
                .bind(detail)
                .execute(self.db.pool()),
            )
            .await;
    }

    // ------------------------------------------------------------ cursors --

    /// Upsert one cursor.
    pub async fn save_cursor(&self, c: &FeedCursor) -> RepoResult<()> {
        self.db
            .timed(
                "ha_cursor_save",
                sqlx::query(
                    r#"INSERT INTO ha_cursors
                           (cursor_key, feed, scope, position, token, last_event_at,
                            processed_count, duplicate_count, gap_count, worker_id, updated_at)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                       ON CONFLICT (cursor_key) DO UPDATE SET
                           position = EXCLUDED.position,
                           token = EXCLUDED.token,
                           last_event_at = EXCLUDED.last_event_at,
                           processed_count = EXCLUDED.processed_count,
                           duplicate_count = EXCLUDED.duplicate_count,
                           gap_count = EXCLUDED.gap_count,
                           worker_id = EXCLUDED.worker_id,
                           updated_at = EXCLUDED.updated_at"#,
                )
                .bind(c.key())
                .bind(c.feed.as_str())
                .bind(&c.scope)
                .bind(c.position.map(|p| p.min(i64::MAX as u64) as i64))
                .bind(&c.token)
                .bind(c.last_event_at)
                .bind(c.processed_count)
                .bind(c.duplicate_count)
                .bind(c.gap_count)
                .bind(&c.worker_id)
                .bind(c.updated_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Load one cursor.
    pub async fn load_cursor(&self, key: &str) -> RepoResult<Option<FeedCursor>> {
        let row = self
            .db
            .timed(
                "ha_cursor_load",
                sqlx::query(
                    r#"SELECT cursor_key, feed, scope, position, token, last_event_at,
                              processed_count, duplicate_count, gap_count, worker_id, updated_at
                         FROM ha_cursors WHERE cursor_key = $1"#,
                )
                .bind(key)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.as_ref().and_then(cursor_from_row))
    }

    /// Every cursor.
    pub async fn cursors(&self) -> RepoResult<Vec<FeedCursor>> {
        let rows = self
            .db
            .timed(
                "ha_cursors_list",
                sqlx::query(
                    r#"SELECT cursor_key, feed, scope, position, token, last_event_at,
                              processed_count, duplicate_count, gap_count, worker_id, updated_at
                         FROM ha_cursors ORDER BY cursor_key"#,
                )
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().filter_map(cursor_from_row).collect())
    }

    // --------------------------------------------------------------- gaps --

    /// Record a detected gap (idempotent on the exact range).
    pub async fn record_gap(&self, g: &FeedGap) -> RepoResult<()> {
        self.db
            .timed(
                "ha_gap_record",
                sqlx::query(
                    r#"INSERT INTO ha_feed_gaps
                           (feed, scope, from_position, to_position, status, worker_id, detected_at)
                       VALUES ($1, $2, $3, $4, $5, $6, $7)
                       ON CONFLICT (feed, scope, from_position, to_position) DO NOTHING"#,
                )
                .bind(g.feed.as_str())
                .bind(&g.scope)
                .bind(g.from_position.min(i64::MAX as u64) as i64)
                .bind(g.to_position.min(i64::MAX as u64) as i64)
                .bind(g.status.as_str())
                .bind(&g.worker_id)
                .bind(g.detected_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Gaps, newest first.
    pub async fn gaps(&self, unresolved_only: bool, limit: i64) -> RepoResult<Vec<FeedGap>> {
        let rows = self
            .db
            .timed(
                "ha_gaps_list",
                sqlx::query(
                    r#"SELECT feed, scope, from_position, to_position, status, worker_id, detected_at
                         FROM ha_feed_gaps
                        WHERE ($1 = false OR status = 'detected')
                        ORDER BY detected_at DESC, id DESC
                        LIMIT $2"#,
                )
                .bind(unresolved_only)
                .bind(limit.clamp(1, 1000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().filter_map(gap_from_row).collect())
    }

    /// Resolve a gap (backfilled / accepted).
    pub async fn resolve_gap(
        &self,
        feed: &str,
        scope: &str,
        from_position: u64,
        status: GapStatus,
    ) -> RepoResult<bool> {
        let res = self
            .db
            .timed(
                "ha_gap_resolve",
                sqlx::query(
                    r#"UPDATE ha_feed_gaps
                          SET status = $4, resolved_at = now()
                        WHERE feed = $1 AND scope = $2 AND from_position = $3
                          AND status = 'detected'"#,
                )
                .bind(feed)
                .bind(scope)
                .bind(from_position.min(i64::MAX as u64) as i64)
                .bind(status.as_str())
                .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected() >= 1)
    }

    // ----------------------------------------------------------- recovery --

    /// Append one recovery record.
    pub async fn record_recovery(&self, r: &RecoveryRecord) -> RepoResult<()> {
        self.db
            .timed(
                "ha_recovery_record",
                sqlx::query(
                    r#"INSERT INTO ha_recovery_records
                           (worker_id, generation, trigger, scope, subject, action, detail, ts)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"#,
                )
                .bind(&r.worker_id)
                .bind(r.generation)
                .bind(&r.trigger)
                .bind(&r.scope)
                .bind(&r.subject)
                .bind(r.action.as_str())
                .bind(&r.detail)
                .bind(r.ts)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Recovery records, newest first.
    pub async fn recovery_records(&self, limit: i64) -> RepoResult<Vec<RecoveryRecord>> {
        let rows = self
            .db
            .timed(
                "ha_recovery_list",
                sqlx::query(
                    r#"SELECT worker_id, generation, trigger, scope, subject, action, detail, ts
                         FROM ha_recovery_records ORDER BY ts DESC, id DESC LIMIT $1"#,
                )
                .bind(limit.clamp(1, 1000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().filter_map(recovery_from_row).collect())
    }
}

fn opt_string(r: &PgRow, col: &str) -> Option<String> {
    r.try_get::<Option<String>, _>(col).ok().flatten()
}

fn ts_or_now(r: &PgRow, col: &str) -> DateTime<Utc> {
    r.try_get(col).unwrap_or_else(|_| Utc::now())
}

fn worker_from_row(r: &PgRow) -> Option<WorkerRegistration> {
    Some(WorkerRegistration {
        worker_id: r.try_get("worker_id").ok()?,
        generation: r.try_get("generation").unwrap_or(1),
        mode: HaMode::parse(&r.try_get::<String, _>("mode").ok()?).unwrap_or(HaMode::Single),
        state: WorkerState::parse(&r.try_get::<String, _>("state").ok()?)
            .unwrap_or(WorkerState::Starting),
        host: r.try_get("host").unwrap_or_default(),
        pid: r.try_get("pid").unwrap_or_default(),
        version: r.try_get("version").unwrap_or_default(),
        started_at: ts_or_now(r, "started_at"),
        last_seen_at: ts_or_now(r, "last_seen_at"),
        detail: r.try_get("detail").unwrap_or_default(),
    })
}

fn lease_from_row(r: &PgRow) -> Option<Lease> {
    Some(Lease {
        role: LeaseRole::parse(&r.try_get::<String, _>("role").ok()?)?,
        holder: r.try_get("holder").ok()?,
        generation: r.try_get("generation").unwrap_or(1),
        acquired_at: ts_or_now(r, "acquired_at"),
        expires_at: ts_or_now(r, "expires_at"),
        renewed_at: ts_or_now(r, "renewed_at"),
        takeover_count: r.try_get("takeover_count").unwrap_or(0),
        previous_holder: opt_string(r, "previous_holder"),
        released: r.try_get("released").unwrap_or(false),
    })
}

fn cursor_from_row(r: &PgRow) -> Option<FeedCursor> {
    Some(FeedCursor {
        feed: FeedId::parse(&r.try_get::<String, _>("feed").ok()?)?,
        scope: r.try_get("scope").unwrap_or_default(),
        position: r
            .try_get::<Option<i64>, _>("position")
            .ok()
            .flatten()
            .map(|p| p.max(0) as u64),
        token: opt_string(r, "token"),
        last_event_at: r
            .try_get::<Option<DateTime<Utc>>, _>("last_event_at")
            .ok()
            .flatten(),
        updated_at: ts_or_now(r, "updated_at"),
        processed_count: r.try_get("processed_count").unwrap_or(0),
        duplicate_count: r.try_get("duplicate_count").unwrap_or(0),
        gap_count: r.try_get("gap_count").unwrap_or(0),
        worker_id: r.try_get("worker_id").unwrap_or_default(),
    })
}

fn gap_from_row(r: &PgRow) -> Option<FeedGap> {
    Some(FeedGap {
        feed: FeedId::parse(&r.try_get::<String, _>("feed").ok()?)?,
        scope: r.try_get("scope").unwrap_or_default(),
        from_position: r.try_get::<i64, _>("from_position").unwrap_or(0).max(0) as u64,
        to_position: r.try_get::<i64, _>("to_position").unwrap_or(0).max(0) as u64,
        detected_at: ts_or_now(r, "detected_at"),
        worker_id: r.try_get("worker_id").unwrap_or_default(),
        status: GapStatus::parse(&r.try_get::<String, _>("status").ok()?)
            .unwrap_or(GapStatus::Detected),
    })
}

fn recovery_from_row(r: &PgRow) -> Option<RecoveryRecord> {
    Some(RecoveryRecord {
        worker_id: r.try_get("worker_id").ok()?,
        generation: r.try_get("generation").unwrap_or(0),
        trigger: r.try_get("trigger").unwrap_or_default(),
        scope: r.try_get("scope").unwrap_or_default(),
        subject: r.try_get("subject").unwrap_or_default(),
        action: OrderRecoveryAction::parse(&r.try_get::<String, _>("action").ok()?)?,
        detail: r.try_get("detail").unwrap_or_default(),
        ts: ts_or_now(r, "ts"),
    })
}
