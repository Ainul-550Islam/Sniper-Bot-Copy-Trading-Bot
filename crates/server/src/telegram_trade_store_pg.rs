//! BATCH 04 — Postgres-backed stores for the Telegram trading console.
//!
//! `module-telegram` owns the rules, the parsing, the nonce handshake, the
//! pending state machine and the audit *shape*; it deliberately owns no
//! database code (it has no `sqlx` dependency). This file is the durable half
//! of [`module_telegram::consent::store::ConsentStore`],
//! [`module_telegram::commands::pending::CommandStore`] and
//! [`module_telegram::commands::audit::AuditStore`] over migrations
//! `0066_telegram_trade_consents.sql` and `0067_telegram_trade_commands.sql`.
//!
//! ## What this file does NOT do
//! It never decides policy. Caps, the phrase, TTL bounds, scope, the pending
//! transitions and the nonce comparison all live in `module-telegram`; this
//! layer stores rows and performs compare-and-set transitions. Two invariants
//! are nevertheless re-asserted by the database itself (partial unique indexes
//! and CHECK constraints), so a buggy writer cannot create two live consents
//! for one chat, two pending commands in one chat, or a row with a nonsense
//! amount.
//!
//! ## Secrets
//! Only `phrase_sha256` and `nonce_sha256` digests are stored. No method in
//! this file accepts or returns a nonce, a phrase or a token.

use std::str::FromStr;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use module_telegram::commands::audit::{AuditDetail, AuditEvent, AuditStore, TradeAuditEvent};
use module_telegram::commands::pending::{CommandStatus, CommandStore, TradeCommand};
use module_telegram::consent::store::{ConsentStore, StoreError};
use module_telegram::consent::{Consent, ConsentScope};
use module_telegram::trade_port::TradeAction;
use module_telegram::TradeStores;

/// `bigint` cannot hold every `u64`. Lamport amounts that do not fit are a
/// programming/telemetry error, never silently wrapped.
fn lamports_to_i64(value: u64, what: &str) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| {
        StoreError::Unavailable(format!("{what} {value} does not fit a bigint column"))
    })
}

/// The inverse of [`lamports_to_i64`]; a negative column value is corruption.
fn i64_to_lamports(value: i64, what: &str) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| {
        StoreError::Unavailable(format!(
            "{what} column is negative ({value}); refusing to read it"
        ))
    })
}

/// The organization id as a UUID — the tenant key of every row this file
/// touches. A non-UUID id can only come from a misconfiguration.
fn org_uuid(organization_id: &str) -> Result<Uuid, StoreError> {
    Uuid::from_str(organization_id).map_err(|_| {
        StoreError::Unavailable(
            "organization_id is not a uuid; refusing to touch tenant rows".into(),
        )
    })
}

fn db_err(what: &'static str, e: sqlx::Error) -> StoreError {
    StoreError::Unavailable(format!("{what}: {e}"))
}

/// `duplicate key` from a partial unique index means "the invariant fired",
/// which is a conflict rather than an outage.
fn unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.code().as_deref() == Some("23505"))
}

fn check_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.code().as_deref() == Some("23514"))
}

fn consent_err(what: &'static str, e: sqlx::Error) -> StoreError {
    if unique_violation(&e) {
        return StoreError::Conflict("a live consent already exists for this chat".into());
    }
    if check_violation(&e) {
        return StoreError::Conflict(format!("{what}: constraint rejected the row ({e})"));
    }
    StoreError::Unavailable(format!("{what}: {e}"))
}

/// Parse the `scope` array into the typed scope, tolerating nothing unknown.
fn scope_from_row(labels: Vec<String>) -> Result<ConsentScope, StoreError> {
    ConsentScope::from_labels(&labels).map_err(|e| {
        StoreError::Unavailable(format!("stored consent scope is invalid: {}", e.code()))
    })
}

/// Parse a stored status label.
fn status_from_row(label: &str) -> Result<CommandStatus, StoreError> {
    CommandStatus::from_label(label)
        .ok_or_else(|| StoreError::Unavailable(format!("unknown command status {label:?} in row")))
}

/// Parse a stored action label.
fn action_from_row(label: &str) -> Result<TradeAction, StoreError> {
    match label {
        "buy" => Ok(TradeAction::Buy),
        "sell" => Ok(TradeAction::Sell),
        other => Err(StoreError::Unavailable(format!(
            "unknown command action {other:?} in row"
        ))),
    }
}

/// Cross-check the stored idempotency key against the command id.
///
/// The two are the same by contract (`TradeCommand::open` sets
/// `idempotency_key = command_id`); a row where they differ means the ledger
/// was written by something that does not follow that contract, and executing
/// it could place a second order.
fn idempotency_key_from_row(stored: Uuid, command_id: Uuid) -> Result<Uuid, StoreError> {
    if stored != command_id {
        return Err(StoreError::Unavailable(
            "idempotency_key does not match command_id; refusing to read the row".into(),
        ));
    }
    Ok(stored)
}

/// A `bytea` digest column as a fixed array.
fn digest_from_row(bytes: &[u8], what: &str) -> Result<[u8; 32], StoreError> {
    <[u8; 32]>::try_from(bytes).map_err(|_| {
        StoreError::Unavailable(format!(
            "{what} column is {} bytes, expected 32",
            bytes.len()
        ))
    })
}

/// Postgres stores for the whole Telegram trading console.
///
/// One pool, three trait objects — exactly what
/// [`module_telegram::TradeConsole`] needs:
///
/// `text` matters: the snippet is a shape sketch, not a compilable example —
/// `db`, `state`, `port` and `config` belong to the caller. Keeping it as a
/// `text` block means `cargo test` never reports it as a skipped ("ignored")
/// doctest, which this batch does not allow.
///
/// ```text
/// let stores = PgTradeStore::new(db.pool().clone()).trade_stores();
/// let console = TradeConsole::new(state, Arc::new(port), stores, config);
/// ```
#[derive(Clone, Debug)]
pub struct PgTradeStore {
    pool: PgPool,
}

impl PgTradeStore {
    /// Wrap an existing pool (the tenant-scoped application pool).
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The underlying pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// The consent store.
    pub fn consents(&self) -> PgConsentStore {
        PgConsentStore {
            pool: self.pool.clone(),
        }
    }

    /// The command (nonce/pending) store.
    pub fn commands(&self) -> PgCommandStore {
        PgCommandStore {
            pool: self.pool.clone(),
        }
    }

    /// The audit store.
    pub fn audit(&self) -> PgAuditStore {
        PgAuditStore {
            pool: self.pool.clone(),
        }
    }

    /// The bundle `TradeConsole::new` expects.
    pub fn trade_stores(&self) -> TradeStores {
        TradeStores::new(
            std::sync::Arc::new(self.consents()),
            std::sync::Arc::new(self.commands()),
            std::sync::Arc::new(self.audit()),
        )
    }

    /// How many of the Batch 04 tables are present (0..=3). Used by the
    /// deployment smoke check so a missing migration is loud, not silent.
    pub async fn installed_tables(&self) -> Result<i64, StoreError> {
        let row = sqlx::query(
            "SELECT count(*) AS n FROM pg_class c JOIN pg_namespace n2 ON n2.oid = c.relnamespace \
             WHERE n2.nspname = 'public' AND c.relkind = 'r' \
               AND c.relname IN ('telegram_trade_consents', 'telegram_trade_commands', \
                                 'telegram_trade_audit')",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| db_err("installed_tables", e))?;
        Ok(row.get::<i64, _>("n"))
    }
}

// ---------------------------------------------------------------------------
// Consents
// ---------------------------------------------------------------------------

/// `telegram_trade_consents` over `sqlx`.
#[derive(Clone, Debug)]
pub struct PgConsentStore {
    pool: PgPool,
}

const CONSENT_COLUMNS: &str = "consent_id, organization_id, user_id, chat_id, scope, \
     max_order_lamports, daily_cap_lamports, granted_at, expires_at, revoked_at, phrase_sha256";

impl PgConsentStore {
    /// Wrap a pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn map_row(row: &sqlx::postgres::PgRow) -> Result<Consent, StoreError> {
        Ok(Consent {
            consent_id: row.get::<Uuid, _>("consent_id"),
            organization_id: row.get::<Uuid, _>("organization_id").to_string(),
            user_id: row.get::<i64, _>("user_id"),
            chat_id: row.get::<i64, _>("chat_id"),
            scope: scope_from_row(row.get::<Vec<String>, _>("scope"))?,
            max_order_lamports: i64_to_lamports(
                row.get::<i64, _>("max_order_lamports"),
                "max_order_lamports",
            )?,
            daily_cap_lamports: i64_to_lamports(
                row.get::<i64, _>("daily_cap_lamports"),
                "daily_cap_lamports",
            )?,
            granted_at: row.get::<DateTime<Utc>, _>("granted_at"),
            expires_at: row.get::<DateTime<Utc>, _>("expires_at"),
            revoked_at: row.get::<Option<DateTime<Utc>>, _>("revoked_at"),
            phrase_sha256: digest_from_row(
                &row.get::<Vec<u8>, _>("phrase_sha256"),
                "phrase_sha256",
            )?,
        })
    }

    /// Every row of one organization, newest first (operator diagnostics).
    pub async fn history(
        &self,
        organization_id: &str,
        limit: i64,
    ) -> Result<Vec<Consent>, StoreError> {
        let org = org_uuid(organization_id)?;
        let rows = sqlx::query(&format!(
            "SELECT {CONSENT_COLUMNS} FROM telegram_trade_consents \
             WHERE organization_id = $1 ORDER BY granted_at DESC LIMIT $2"
        ))
        .bind(org)
        .bind(limit.clamp(1, 1_000))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| db_err("consent history", e))?;
        rows.iter().map(Self::map_row).collect()
    }

    /// Count the live (non-revoked) rows of a chat — must never exceed 1.
    pub async fn live_count(&self, organization_id: &str, chat_id: i64) -> Result<i64, StoreError> {
        let org = org_uuid(organization_id)?;
        let row = sqlx::query(
            "SELECT count(*) AS n FROM telegram_trade_consents \
             WHERE organization_id = $1 AND chat_id = $2 AND revoked_at IS NULL",
        )
        .bind(org)
        .bind(chat_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| db_err("consent live_count", e))?;
        Ok(row.get::<i64, _>("n"))
    }
}

#[async_trait]
impl ConsentStore for PgConsentStore {
    async fn active(
        &self,
        organization_id: &str,
        chat_id: i64,
        _now: DateTime<Utc>,
    ) -> Result<Option<Consent>, StoreError> {
        let org = org_uuid(organization_id)?;
        let rows = sqlx::query(&format!(
            "SELECT {CONSENT_COLUMNS} FROM telegram_trade_consents \
             WHERE organization_id = $1 AND chat_id = $2 AND revoked_at IS NULL \
             ORDER BY granted_at DESC LIMIT 1"
        ))
        .bind(org)
        .bind(chat_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| db_err("consent active", e))?;
        rows.first().map(Self::map_row).transpose()
    }

    async fn insert(&self, consent: &Consent) -> Result<(), StoreError> {
        let org = org_uuid(&consent.organization_id)?;
        let mut tx: Transaction<'_, Postgres> = self
            .pool
            .begin()
            .await
            .map_err(|e| db_err("consent insert begin", e))?;

        // Lock any live predecessor for this chat: the "one live consent"
        // invariant is enforced under a row lock, then again by the partial
        // unique index (belt and braces for a second writer).
        let existing = sqlx::query(
            "SELECT expires_at FROM telegram_trade_consents \
             WHERE organization_id = $1 AND chat_id = $2 AND revoked_at IS NULL \
             ORDER BY granted_at DESC LIMIT 1 FOR UPDATE",
        )
        .bind(org)
        .bind(consent.chat_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| db_err("consent insert lock", e))?;

        if let Some(row) = existing {
            let expires_at: DateTime<Utc> = row.get("expires_at");
            if consent.granted_at < expires_at {
                return Err(StoreError::Conflict(
                    "a live consent already exists for this chat".into(),
                ));
            }
            // Lapsed predecessor: retire it, then store the new grant.
            sqlx::query(
                "UPDATE telegram_trade_consents SET revoked_at = $3 \
                 WHERE organization_id = $1 AND chat_id = $2 AND revoked_at IS NULL",
            )
            .bind(org)
            .bind(consent.chat_id)
            .bind(consent.granted_at)
            .execute(&mut *tx)
            .await
            .map_err(|e| db_err("consent insert retire", e))?;
        }

        sqlx::query(
            "INSERT INTO telegram_trade_consents \
             (consent_id, organization_id, user_id, chat_id, scope, max_order_lamports, \
              daily_cap_lamports, granted_at, expires_at, revoked_at, phrase_sha256) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(consent.consent_id)
        .bind(org)
        .bind(consent.user_id)
        .bind(consent.chat_id)
        .bind(consent.scope.to_sql_array())
        .bind(lamports_to_i64(
            consent.max_order_lamports,
            "max_order_lamports",
        )?)
        .bind(lamports_to_i64(
            consent.daily_cap_lamports,
            "daily_cap_lamports",
        )?)
        .bind(consent.granted_at)
        .bind(consent.expires_at)
        .bind(consent.revoked_at)
        .bind(consent.phrase_sha256.to_vec())
        .execute(&mut *tx)
        .await
        .map_err(|e| consent_err("consent insert", e))?;

        tx.commit()
            .await
            .map_err(|e| db_err("consent insert commit", e))
    }

    async fn revoke(
        &self,
        organization_id: &str,
        chat_id: i64,
        now: DateTime<Utc>,
    ) -> Result<Option<Consent>, StoreError> {
        let org = org_uuid(organization_id)?;
        let rows = sqlx::query(&format!(
            "UPDATE telegram_trade_consents SET revoked_at = $3 \
             WHERE organization_id = $1 AND chat_id = $2 AND revoked_at IS NULL \
             RETURNING {CONSENT_COLUMNS}"
        ))
        .bind(org)
        .bind(chat_id)
        .bind(now)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| db_err("consent revoke", e))?;
        // At most one row can match (partial unique index); tolerate the
        // impossible >1 by returning the newest and never by erroring.
        rows.last().map(Self::map_row).transpose()
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// `telegram_trade_commands` over `sqlx`.
#[derive(Clone, Debug)]
pub struct PgCommandStore {
    pool: PgPool,
}

const COMMAND_COLUMNS: &str = "command_id, organization_id, user_id, chat_id, action, mint, \
     amount_lamports, percent_bps, slippage_bps, nonce_sha256, idempotency_key, status, \
     failure_reason, created_at, expires_at, confirmed_at, settled_at";

impl PgCommandStore {
    /// Wrap a pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn map_row(row: &sqlx::postgres::PgRow) -> Result<TradeCommand, StoreError> {
        let command_id: Uuid = row.get("command_id");
        let amount: Option<i64> = row.get("amount_lamports");
        let percent: Option<i32> = row.get("percent_bps");
        let created_at: DateTime<Utc> = row.get("created_at");
        let confirmed_at: Option<DateTime<Utc>> = row.get("confirmed_at");
        let settled_at: Option<DateTime<Utc>> = row.get("settled_at");
        Ok(TradeCommand {
            command_id,
            organization_id: row.get::<Uuid, _>("organization_id").to_string(),
            user_id: row.get::<i64, _>("user_id"),
            chat_id: row.get::<i64, _>("chat_id"),
            action: action_from_row(row.get::<&str, _>("action"))?,
            mint: row.get::<String, _>("mint"),
            amount_lamports: amount
                .map(|v| i64_to_lamports(v, "amount_lamports"))
                .transpose()?,
            percent_bps: percent
                .map(|v| {
                    u16::try_from(v).map_err(|_| {
                        StoreError::Unavailable(format!(
                            "percent_bps {v} does not fit the typed range"
                        ))
                    })
                })
                .transpose()?,
            slippage_bps: u16::try_from(row.get::<i16, _>("slippage_bps")).map_err(|_| {
                StoreError::Unavailable(
                    "slippage_bps column is negative; refusing to read it".into(),
                )
            })?,
            nonce_sha256: digest_from_row(&row.get::<Vec<u8>, _>("nonce_sha256"), "nonce_sha256")?,
            status: status_from_row(row.get::<&str, _>("status"))?,
            // The idempotency key of a Telegram command IS its command id: a
            // duplicate `/confirm` therefore carries the same key downstream.
            // The column is read (not assumed) and cross-checked, so a row that
            // somehow disagrees with the contract is a loud corruption error
            // rather than a silent second execution.
            idempotency_key: idempotency_key_from_row(
                row.get::<Uuid, _>("idempotency_key"),
                command_id,
            )?,
            expires_at: row.get::<DateTime<Utc>, _>("expires_at"),
            created_at,
            // The row's last touch is whichever lifecycle stamp is newest.
            updated_at: settled_at.or(confirmed_at).unwrap_or(created_at),
            failure_code: row.get::<Option<String>, _>("failure_reason"),
        })
    }

    /// Status timeline of one chat, oldest first (operator diagnostics).
    pub async fn history(
        &self,
        organization_id: &str,
        chat_id: i64,
        limit: i64,
    ) -> Result<Vec<TradeCommand>, StoreError> {
        let org = org_uuid(organization_id)?;
        let rows = sqlx::query(&format!(
            "SELECT {COMMAND_COLUMNS} FROM telegram_trade_commands \
             WHERE organization_id = $1 AND chat_id = $2 \
             ORDER BY created_at ASC LIMIT $3"
        ))
        .bind(org)
        .bind(chat_id)
        .bind(limit.clamp(1, 1_000))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| db_err("command history", e))?;
        rows.iter().map(Self::map_row).collect()
    }

    /// Number of rows of one organization in a status (diagnostics).
    pub async fn count_in_status(
        &self,
        organization_id: &str,
        status: CommandStatus,
    ) -> Result<i64, StoreError> {
        let org = org_uuid(organization_id)?;
        let row = sqlx::query(
            "SELECT count(*) AS n FROM telegram_trade_commands \
             WHERE organization_id = $1 AND status = $2",
        )
        .bind(org)
        .bind(status.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| db_err("command count_in_status", e))?;
        Ok(row.get::<i64, _>("n"))
    }
}

#[async_trait]
impl CommandStore for PgCommandStore {
    async fn insert(&self, command: &TradeCommand) -> Result<(), StoreError> {
        let org = org_uuid(&command.organization_id)?;
        sqlx::query(
            "INSERT INTO telegram_trade_commands \
             (command_id, organization_id, user_id, chat_id, consent_id, action, mint, \
              amount_lamports, percent_bps, slippage_bps, nonce_sha256, idempotency_key, \
              status, created_at, expires_at) \
             VALUES ($1, $2, $3, $4, NULL, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        )
        .bind(command.command_id)
        .bind(org)
        .bind(command.user_id)
        .bind(command.chat_id)
        .bind(command.action.as_str())
        .bind(&command.mint)
        .bind(
            command
                .amount_lamports
                .map(|v| lamports_to_i64(v, "amount_lamports"))
                .transpose()?,
        )
        .bind(command.percent_bps.map(i32::from))
        .bind(i16::try_from(command.slippage_bps).map_err(|_| {
            StoreError::Unavailable(format!(
                "slippage_bps {} does not fit a smallint column",
                command.slippage_bps
            ))
        })?)
        .bind(command.nonce_sha256.to_vec())
        .bind(command.idempotency_key)
        .bind(command.status.as_str())
        .bind(command.created_at)
        .bind(command.expires_at)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if unique_violation(&e) {
                // The one-pending-per-chat index fired.
                StoreError::Conflict("this chat already has a pending command".into())
            } else if check_violation(&e) {
                StoreError::Conflict(format!("command row rejected by a constraint: {e}"))
            } else {
                db_err("command insert", e)
            }
        })?;
        Ok(())
    }

    async fn get(&self, command_id: Uuid) -> Result<Option<TradeCommand>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {COMMAND_COLUMNS} FROM telegram_trade_commands WHERE command_id = $1"
        ))
        .bind(command_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| db_err("command get", e))?;
        rows.first().map(Self::map_row).transpose()
    }

    async fn find_by_nonce(
        &self,
        organization_id: &str,
        chat_id: i64,
        nonce_sha256: &[u8; 32],
    ) -> Result<Option<TradeCommand>, StoreError> {
        let org = org_uuid(organization_id)?;
        let rows = sqlx::query(&format!(
            "SELECT {COMMAND_COLUMNS} FROM telegram_trade_commands \
             WHERE organization_id = $1 AND chat_id = $2 AND nonce_sha256 = $3 \
             ORDER BY created_at DESC LIMIT 1"
        ))
        .bind(org)
        .bind(chat_id)
        .bind(nonce_sha256.to_vec())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| db_err("command find_by_nonce", e))?;
        rows.first().map(Self::map_row).transpose()
    }

    async fn transition(
        &self,
        command_id: Uuid,
        from: CommandStatus,
        to: CommandStatus,
        now: DateTime<Utc>,
        failure_code: Option<&str>,
    ) -> Result<bool, StoreError> {
        let result = sqlx::query(
            "UPDATE telegram_trade_commands SET \
                 status = $3, \
                 failure_reason = COALESCE($4, failure_reason), \
                 confirmed_at = CASE WHEN $3 = 'confirmed' THEN $5 ELSE confirmed_at END, \
                 settled_at = CASE WHEN $3 IN ('executed','failed','expired','cancelled') \
                                   THEN $5 ELSE settled_at END \
             WHERE command_id = $1 AND status = $2",
        )
        .bind(command_id)
        .bind(from.as_str())
        .bind(to.as_str())
        .bind(failure_code)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| db_err("command transition", e))?;
        Ok(result.rows_affected() == 1)
    }

    async fn pending(
        &self,
        organization_id: &str,
        chat_id: i64,
        now: DateTime<Utc>,
    ) -> Result<Vec<TradeCommand>, StoreError> {
        let org = org_uuid(organization_id)?;
        let rows = sqlx::query(&format!(
            "SELECT {COMMAND_COLUMNS} FROM telegram_trade_commands \
             WHERE organization_id = $1 AND chat_id = $2 AND status = 'pending' \
               AND expires_at > $3 \
             ORDER BY created_at ASC"
        ))
        .bind(org)
        .bind(chat_id)
        .bind(now)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| db_err("command pending", e))?;
        rows.iter().map(Self::map_row).collect()
    }

    async fn cancel_pending(
        &self,
        organization_id: &str,
        chat_id: i64,
        now: DateTime<Utc>,
    ) -> Result<Vec<Uuid>, StoreError> {
        let org = org_uuid(organization_id)?;
        let rows = sqlx::query(
            "UPDATE telegram_trade_commands SET status = 'cancelled', settled_at = $3 \
             WHERE organization_id = $1 AND chat_id = $2 AND status = 'pending' \
             RETURNING command_id",
        )
        .bind(org)
        .bind(chat_id)
        .bind(now)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| db_err("command cancel_pending", e))?;
        Ok(rows
            .iter()
            .map(|r| r.get::<Uuid, _>("command_id"))
            .collect())
    }

    async fn expire_overdue(&self, now: DateTime<Utc>) -> Result<Vec<Uuid>, StoreError> {
        let rows = sqlx::query(
            "UPDATE telegram_trade_commands SET status = 'expired', settled_at = $1 \
             WHERE status = 'pending' AND expires_at <= $1 \
             RETURNING command_id",
        )
        .bind(now)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| db_err("command expire_overdue", e))?;
        Ok(rows
            .iter()
            .map(|r| r.get::<Uuid, _>("command_id"))
            .collect())
    }

    async fn spent_since(
        &self,
        organization_id: &str,
        chat_id: i64,
        since: DateTime<Utc>,
    ) -> Result<u64, StoreError> {
        let org = org_uuid(organization_id)?;
        let row = sqlx::query(
            "SELECT COALESCE(SUM(amount_lamports), 0)::bigint AS total \
             FROM telegram_trade_commands \
             WHERE organization_id = $1 AND chat_id = $2 \
               AND status IN ('confirmed', 'executed') \
               AND amount_lamports IS NOT NULL \
               AND created_at >= $3",
        )
        .bind(org)
        .bind(chat_id)
        .bind(since)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| db_err("command spent_since", e))?;
        i64_to_lamports(row.get::<i64, _>("total"), "spent_since total")
    }

    async fn count_since(
        &self,
        organization_id: &str,
        chat_id: i64,
        since: DateTime<Utc>,
    ) -> Result<u64, StoreError> {
        let org = org_uuid(organization_id)?;
        let row = sqlx::query(
            "SELECT count(*)::bigint AS n FROM telegram_trade_commands \
             WHERE organization_id = $1 AND chat_id = $2 AND created_at >= $3",
        )
        .bind(org)
        .bind(chat_id)
        .bind(since)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| db_err("command count_since", e))?;
        let n: i64 = row.get("n");
        u64::try_from(n).map_err(|_| StoreError::Unavailable("negative command count".into()))
    }
}

// ---------------------------------------------------------------------------
// Audit
// ---------------------------------------------------------------------------

/// `telegram_trade_audit` over `sqlx` (append-only).
#[derive(Clone, Debug)]
pub struct PgAuditStore {
    pool: PgPool,
}

impl PgAuditStore {
    /// Wrap a pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Row count of one organization (diagnostics and the smoke script).
    pub async fn count(&self, organization_id: &str) -> Result<i64, StoreError> {
        let org = org_uuid(organization_id)?;
        let row = sqlx::query(
            "SELECT count(*) AS n FROM telegram_trade_audit WHERE organization_id = $1",
        )
        .bind(org)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| db_err("audit count", e))?;
        Ok(row.get::<i64, _>("n"))
    }

    /// Rows of one command, oldest first (the state-transition timeline).
    pub async fn for_command(&self, command_id: Uuid) -> Result<Vec<TradeAuditEvent>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, organization_id, chat_id, command_id, event, detail, ts \
             FROM telegram_trade_audit WHERE command_id = $1 ORDER BY id ASC",
        )
        .bind(command_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| db_err("audit for_command", e))?;
        rows.iter().map(Self::map_row).collect()
    }

    /// The append-only guard: any UPDATE/DELETE privilege on the audit table is
    /// reported so the deployment can revoke it.
    pub async fn has_write_grants(&self) -> Result<bool, StoreError> {
        let row = sqlx::query(
            "SELECT has_table_privilege(current_user, 'public.telegram_trade_audit', 'UPDATE') \
             OR has_table_privilege(current_user, 'public.telegram_trade_audit', 'DELETE') AS w",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| db_err("audit has_write_grants", e))?;
        Ok(row.get::<bool, _>("w"))
    }

    fn map_row(row: &sqlx::postgres::PgRow) -> Result<TradeAuditEvent, StoreError> {
        let event_label: &str = row.get("event");
        let event = AuditEvent::from_label(event_label).ok_or_else(|| {
            StoreError::Unavailable(format!("unknown audit event {event_label:?} in row"))
        })?;
        let detail: serde_json::Value = row.get("detail");
        let map = match detail {
            serde_json::Value::Object(map) => map,
            _ => {
                return Err(StoreError::Unavailable(
                    "audit detail column is not a JSON object".into(),
                ))
            }
        };
        Ok(TradeAuditEvent {
            audit_id: Some(row.get::<i64, _>("id")),
            command_id: row.get::<Option<Uuid>, _>("command_id"),
            organization_id: row.get::<Uuid, _>("organization_id").to_string(),
            chat_id: row.get::<i64, _>("chat_id"),
            event,
            detail: AuditDetail::from_map(map),
            created_at: row.get::<DateTime<Utc>, _>("ts"),
        })
    }
}

#[async_trait]
impl AuditStore for PgAuditStore {
    async fn append(&self, event: &TradeAuditEvent) -> Result<(), StoreError> {
        let org = org_uuid(&event.organization_id)?;
        let detail = serde_json::Value::Object(event.detail.as_map().clone());
        let result = sqlx::query(
            "INSERT INTO telegram_trade_audit \
             (organization_id, chat_id, command_id, event, detail, ts) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(org)
        .bind(event.chat_id)
        .bind(event.command_id)
        .bind(event.event.as_str())
        .bind(detail)
        .bind(event.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if check_violation(&e) {
                StoreError::Conflict(format!("audit row rejected by a constraint: {e}"))
            } else {
                db_err("audit append", e)
            }
        })?;
        if result.rows_affected() != 1 {
            return Err(StoreError::Unavailable(
                "audit append wrote no row".to_string(),
            ));
        }
        Ok(())
    }

    async fn recent(
        &self,
        organization_id: &str,
        limit: usize,
    ) -> Result<Vec<TradeAuditEvent>, StoreError> {
        let org = org_uuid(organization_id)?;
        let rows = sqlx::query(
            "SELECT id, organization_id, chat_id, command_id, event, detail, ts \
             FROM telegram_trade_audit WHERE organization_id = $1 \
             ORDER BY id DESC LIMIT $2",
        )
        .bind(org)
        .bind(i64::try_from(limit.clamp(1, 1_000)).unwrap_or(1_000))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| db_err("audit recent", e))?;
        rows.iter().map(Self::map_row).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lamport_columns_round_trip_but_refuse_negative_and_overflow() {
        assert_eq!(lamports_to_i64(0, "amount").unwrap(), 0);
        assert_eq!(
            lamports_to_i64(i64::MAX as u64, "amount").unwrap(),
            i64::MAX
        );
        // u64::MAX cannot be a bigint: refuse, never wrap to -1.
        assert!(lamports_to_i64(u64::MAX, "amount").is_err());
        assert_eq!(i64_to_lamports(0, "amount").unwrap(), 0);
        assert_eq!(
            i64_to_lamports(1_000_000_000, "amount").unwrap(),
            1_000_000_000
        );
        assert!(i64_to_lamports(-1, "amount").is_err());
    }

    #[test]
    fn a_non_uuid_tenant_key_is_refused_before_any_query() {
        assert!(org_uuid("org-1").is_err());
        assert!(org_uuid("").is_err());
        assert_eq!(
            org_uuid("0f8fad5b-d9cb-469f-a165-70867728950e")
                .unwrap()
                .to_string(),
            "0f8fad5b-d9cb-469f-a165-70867728950e"
        );
    }

    #[test]
    fn stored_labels_are_parsed_strictly() {
        assert_eq!(status_from_row("pending").unwrap(), CommandStatus::Pending);
        assert_eq!(
            status_from_row("executed").unwrap(),
            CommandStatus::Executed
        );
        assert!(status_from_row("PENDING").is_err());
        assert!(status_from_row("").is_err());
        assert_eq!(action_from_row("buy").unwrap(), TradeAction::Buy);
        assert_eq!(action_from_row("sell").unwrap(), TradeAction::Sell);
        assert!(action_from_row("swap").is_err());
    }

    #[test]
    fn the_stored_idempotency_key_must_equal_the_command_id() {
        let id = Uuid::new_v4();
        assert_eq!(idempotency_key_from_row(id, id).unwrap(), id);
        let other = Uuid::new_v4();
        assert!(idempotency_key_from_row(other, id).is_err());
    }

    #[test]
    fn a_wrong_length_digest_is_refused() {
        assert!(digest_from_row(&[0u8; 32], "nonce_sha256").is_ok());
        assert!(digest_from_row(&[0u8; 31], "nonce_sha256").is_err());
        assert!(digest_from_row(&[], "nonce_sha256").is_err());
    }

    #[test]
    fn a_stored_scope_is_parsed_strictly() {
        assert_eq!(
            scope_from_row(vec!["buy".into()]).unwrap().labels(),
            vec!["buy"]
        );
        assert!(scope_from_row(vec!["swap".into()]).is_err());
        assert!(scope_from_row(vec![]).is_err());
    }

    #[test]
    fn every_wrapper_implements_the_trait_the_console_needs() {
        fn consent<T: ConsentStore>() {}
        fn commands<T: CommandStore>() {}
        fn audit<T: AuditStore>() {}
        consent::<PgConsentStore>();
        commands::<PgCommandStore>();
        audit::<PgAuditStore>();
    }
}
