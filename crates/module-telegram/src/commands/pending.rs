//! The **only** owner of the pending-command state machine.
//!
//! ```text
//!                    /buy | /sell
//!                          │
//!                     requested
//!                          ▼
//!                     ┌─────────┐  /cancel   ┌───────────┐
//!                     │ pending │───────────▶│ cancelled │
//!                     └────┬────┘            └───────────┘
//!              /confirm    │      TTL elapsed
//!                          │      ┌───────────┐
//!                          ├─────▶│  expired  │
//!                          ▼      └───────────┘
//!                     ┌───────────┐
//!                     │ confirmed │  (idempotency key = command_id)
//!                     └─────┬─────┘
//!                 executed  │  port error
//!                     ┌─────▼─────┐     ┌────────┐
//!                     │ executed  │     │ failed │
//!                     └───────────┘     └────────┘
//! ```
//!
//! Guarantees:
//!
//! * a command is **bound** to one `(chat_id, user_id)` pair and to the exact
//!   parameters frozen at request time — a confirmation carries only the
//!   nonce, never the order;
//! * the transition out of `pending` is a compare-and-set, so two racing
//!   confirmations cannot both win;
//! * every terminal state is final: a duplicate `/confirm` after `executed`
//!   is answered, never re-executed;
//! * the nonce plaintext is never stored — the row keeps
//!   [`TradeCommand::nonce_sha256`] only.
//!
//! [`CommandStore`] is the persistence seam (in-memory here, Postgres in
//! `sniper_suite::telegram_trade_store_pg`, table `telegram_trade_commands`
//! from migration 0067).

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use crate::commands::nonce::Nonce;
use crate::consent::store::StoreError;
use crate::trade_port::TradeAction;

/// How long a pending command stays confirmable.
pub const COMMAND_TTL_SECS: i64 = 60;

/// Lifecycle state persisted in `telegram_trade_commands.status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandStatus {
    /// Awaiting `/confirm`; the only state a nonce can act on.
    Pending,
    /// Confirmed and handed to the port; execution outcome not yet known.
    Confirmed,
    /// The port accepted the order.
    Executed,
    /// The port rejected or failed the order.
    Failed,
    /// The TTL elapsed before confirmation.
    Expired,
    /// The requester (or a consent revocation) cancelled it.
    Cancelled,
    /// Refused before it could ever be confirmed (kept for schema
    /// completeness; the live flow audits pre-creation denials instead).
    Denied,
}

impl CommandStatus {
    /// Stable label (matches the SQL `CHECK` list exactly).
    pub fn as_str(&self) -> &'static str {
        match self {
            CommandStatus::Pending => "pending",
            CommandStatus::Confirmed => "confirmed",
            CommandStatus::Executed => "executed",
            CommandStatus::Failed => "failed",
            CommandStatus::Expired => "expired",
            CommandStatus::Cancelled => "cancelled",
            CommandStatus::Denied => "denied",
        }
    }

    /// Parse a persisted label. Unknown labels are `None` so a corrupted row
    /// can never be treated as pending.
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "pending" => Some(CommandStatus::Pending),
            "confirmed" => Some(CommandStatus::Confirmed),
            "executed" => Some(CommandStatus::Executed),
            "failed" => Some(CommandStatus::Failed),
            "expired" => Some(CommandStatus::Expired),
            "cancelled" => Some(CommandStatus::Cancelled),
            "denied" => Some(CommandStatus::Denied),
            _ => None,
        }
    }

    /// `true` once the command can never move again.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            CommandStatus::Executed
                | CommandStatus::Failed
                | CommandStatus::Expired
                | CommandStatus::Cancelled
                | CommandStatus::Denied
        )
    }
}

/// One row of `telegram_trade_commands`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeCommand {
    /// Primary key; also the port idempotency key.
    pub command_id: Uuid,
    /// Tenant that owns the chat binding.
    pub organization_id: String,
    /// Telegram user that requested it (only this user may confirm).
    pub user_id: i64,
    /// Telegram chat that requested it (only this chat may confirm).
    pub chat_id: i64,
    /// Buy or sell.
    pub action: TradeAction,
    /// Base58 mint, frozen at request time.
    pub mint: String,
    /// Buy amount in lamports.
    pub amount_lamports: Option<u64>,
    /// Sell fraction in basis points.
    pub percent_bps: Option<u16>,
    /// Slippage tolerance in basis points.
    pub slippage_bps: u16,
    /// SHA-256 of the confirmation nonce (the plaintext is never stored).
    pub nonce_sha256: [u8; 32],
    /// Current lifecycle state.
    pub status: CommandStatus,
    /// Key handed to the port; always equal to `command_id`.
    pub idempotency_key: Uuid,
    /// When the nonce stops being usable.
    pub expires_at: DateTime<Utc>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last transition timestamp.
    pub updated_at: DateTime<Utc>,
    /// Machine code of the failure, when `status == Failed`.
    pub failure_code: Option<String>,
}

/// Everything needed to open a pending command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSpec {
    /// Tenant.
    pub organization_id: String,
    /// Requesting Telegram user.
    pub user_id: i64,
    /// Requesting Telegram chat.
    pub chat_id: i64,
    /// Buy or sell.
    pub action: TradeAction,
    /// Validated mint.
    pub mint: String,
    /// Validated buy amount in lamports.
    pub amount_lamports: Option<u64>,
    /// Validated sell fraction in basis points.
    pub percent_bps: Option<u16>,
    /// Validated slippage in basis points.
    pub slippage_bps: u16,
}

impl TradeCommand {
    /// Open a new pending command, returning the row **and** the plaintext
    /// nonce that must be delivered to the chat exactly once.
    pub fn open(spec: PendingSpec, now: DateTime<Utc>) -> (Self, Nonce) {
        let nonce = Nonce::generate();
        let command_id = Uuid::new_v4();
        let row = TradeCommand {
            command_id,
            organization_id: spec.organization_id,
            user_id: spec.user_id,
            chat_id: spec.chat_id,
            action: spec.action,
            mint: spec.mint,
            amount_lamports: spec.amount_lamports,
            percent_bps: spec.percent_bps,
            slippage_bps: spec.slippage_bps,
            nonce_sha256: nonce.sha256(),
            status: CommandStatus::Pending,
            idempotency_key: command_id,
            expires_at: now + Duration::seconds(COMMAND_TTL_SECS),
            created_at: now,
            updated_at: now,
            failure_code: None,
        };
        (row, nonce)
    }

    /// `true` when the nonce can no longer be used.
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at <= now
    }

    /// The SOL amount in lamports, or `0` for a sell.
    pub fn amount_lamports_or_zero(&self) -> u64 {
        self.amount_lamports.unwrap_or(0)
    }

    /// The basis-point fraction, or `0` for a buy.
    pub fn percent_bps_or_zero(&self) -> u16 {
        self.percent_bps.unwrap_or(0)
    }
}

/// A refused transition. Typed so a racing confirmation can be answered
/// idempotently instead of executing twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionError {
    /// The command is already in a terminal state; `current` says which.
    AlreadyResolved(CommandStatus),
    /// The TTL elapsed before the transition.
    Expired,
    /// The transition is not part of the machine at all.
    Illegal {
        /// Source state.
        from: CommandStatus,
        /// Requested target state.
        to: CommandStatus,
    },
}

impl TransitionError {
    /// Stable machine code.
    pub fn code(&self) -> &'static str {
        match self {
            TransitionError::AlreadyResolved(_) => "already_resolved",
            TransitionError::Expired => "expired",
            TransitionError::Illegal { .. } => "illegal_transition",
        }
    }
}

/// `pending → confirmed`, requiring the TTL to still be open.
pub fn transition_confirm(
    command: &TradeCommand,
    now: DateTime<Utc>,
) -> Result<(), TransitionError> {
    match command.status {
        CommandStatus::Pending => {
            if command.is_expired(now) {
                Err(TransitionError::Expired)
            } else {
                Ok(())
            }
        }
        other if other.is_terminal() => Err(TransitionError::AlreadyResolved(other)),
        other => Err(TransitionError::Illegal {
            from: other,
            to: CommandStatus::Confirmed,
        }),
    }
}

/// `pending → cancelled`.
pub fn transition_cancel(
    command: &TradeCommand,
    now: DateTime<Utc>,
) -> Result<(), TransitionError> {
    match command.status {
        CommandStatus::Pending => {
            if command.is_expired(now) {
                Err(TransitionError::Expired)
            } else {
                Ok(())
            }
        }
        other if other.is_terminal() => Err(TransitionError::AlreadyResolved(other)),
        other => Err(TransitionError::Illegal {
            from: other,
            to: CommandStatus::Cancelled,
        }),
    }
}

/// `pending → expired` (TTL housekeeping).
pub fn transition_expired(
    command: &TradeCommand,
    now: DateTime<Utc>,
) -> Result<(), TransitionError> {
    match command.status {
        CommandStatus::Pending => {
            if command.is_expired(now) || now >= command.expires_at {
                Ok(())
            } else {
                Err(TransitionError::Illegal {
                    from: CommandStatus::Pending,
                    to: CommandStatus::Expired,
                })
            }
        }
        other if other.is_terminal() => Err(TransitionError::AlreadyResolved(other)),
        other => Err(TransitionError::Illegal {
            from: other,
            to: CommandStatus::Expired,
        }),
    }
}

/// `confirmed → executed`.
pub fn transition_executed(
    command: &TradeCommand,
    _now: DateTime<Utc>,
) -> Result<(), TransitionError> {
    match command.status {
        CommandStatus::Confirmed => Ok(()),
        other if other.is_terminal() => Err(TransitionError::AlreadyResolved(other)),
        other => Err(TransitionError::Illegal {
            from: other,
            to: CommandStatus::Executed,
        }),
    }
}

/// `confirmed → failed`.
pub fn transition_failed(
    command: &TradeCommand,
    _now: DateTime<Utc>,
) -> Result<(), TransitionError> {
    match command.status {
        CommandStatus::Confirmed => Ok(()),
        other if other.is_terminal() => Err(TransitionError::AlreadyResolved(other)),
        other => Err(TransitionError::Illegal {
            from: other,
            to: CommandStatus::Failed,
        }),
    }
}

/// `confirmed → executed | failed`, chosen by the caller after the port has
/// answered. `Executed` and `Failed` are the only legal targets.
pub fn transition_execute_or_fail(
    command: &TradeCommand,
    target: CommandStatus,
) -> Result<(), TransitionError> {
    match target {
        CommandStatus::Executed => transition_executed(command, Utc::now()),
        CommandStatus::Failed => transition_failed(command, Utc::now()),
        other => Err(TransitionError::Illegal {
            from: command.status,
            to: other,
        }),
    }
}

/// How long a command may sit in `confirmed` before a repeated `/confirm` is
/// treated as **crash recovery** rather than a duplicate tap.
///
/// The distinction matters: inside the grace window the first confirmation is
/// still calling the port, so a second `/confirm` must answer "in flight" and
/// stay its hand. After it, the process that flipped the row evidently died
/// between the transition and the execution, so replaying is correct — and
/// safe, because the replay reuses `idempotency_key` (the command id) and the
/// executing side deduplicates on it.
pub const CONFIRM_GRACE_SECS: i64 = 5;

/// `true` when a `confirmed` row may be replayed to the port.
///
/// A row that is not `confirmed` is never replayable here: `pending` rows go
/// through [`transition_confirm`], terminal rows are final.
pub fn is_confirm_retryable(command: &TradeCommand, now: DateTime<Utc>) -> bool {
    command.status == CommandStatus::Confirmed
        && (now - command.updated_at).num_seconds() >= CONFIRM_GRACE_SECS
}

/// The persistence seam for commands.
#[async_trait]
pub trait CommandStore: Send + Sync {
    /// Insert a freshly opened pending command.
    async fn insert(&self, command: &TradeCommand) -> Result<(), StoreError>;

    /// Fetch by primary key.
    async fn get(&self, command_id: Uuid) -> Result<Option<TradeCommand>, StoreError>;

    /// Find the command in `(organization, chat)` whose nonce digest matches.
    ///
    /// The digest lookup is the only way a confirmation is resolved: the chat
    /// supplies a nonce, the store finds the row, the machine decides.
    async fn find_by_nonce(
        &self,
        organization_id: &str,
        chat_id: i64,
        nonce_sha256: &[u8; 32],
    ) -> Result<Option<TradeCommand>, StoreError>;

    /// Compare-and-set a status transition. Returns `false` when the row was
    /// not in `from` (another confirmation/cancellation won the race).
    async fn transition(
        &self,
        command_id: Uuid,
        from: CommandStatus,
        to: CommandStatus,
        now: DateTime<Utc>,
        failure_code: Option<&str>,
    ) -> Result<bool, StoreError>;

    /// Pending, unexpired commands of a chat, oldest first.
    async fn pending(
        &self,
        organization_id: &str,
        chat_id: i64,
        now: DateTime<Utc>,
    ) -> Result<Vec<TradeCommand>, StoreError>;

    /// Cancel every pending command of a chat, returning the affected ids
    /// (used by `/consent revoke` and by chat unbinding).
    async fn cancel_pending(
        &self,
        organization_id: &str,
        chat_id: i64,
        now: DateTime<Utc>,
    ) -> Result<Vec<Uuid>, StoreError>;

    /// Move every overdue pending command to `expired`, returning the ids.
    async fn expire_overdue(&self, now: DateTime<Utc>) -> Result<Vec<Uuid>, StoreError>;

    /// Lamports committed by BUY commands of this chat since `since`
    /// (statuses `confirmed` and `executed` count — money is committed the
    /// moment the port is called).
    async fn spent_since(
        &self,
        organization_id: &str,
        chat_id: i64,
        since: DateTime<Utc>,
    ) -> Result<u64, StoreError>;

    /// Number of commands of this chat created since `since` (any status) —
    /// the durable half of the rate limiter.
    async fn count_since(
        &self,
        organization_id: &str,
        chat_id: i64,
        since: DateTime<Utc>,
    ) -> Result<u64, StoreError>;
}

/// `true` when this status consumes daily-cap capacity.
fn consumes_capacity(status: CommandStatus) -> bool {
    matches!(status, CommandStatus::Confirmed | CommandStatus::Executed)
}

/// A process-local [`CommandStore`].
#[derive(Debug, Default)]
pub struct InMemoryCommandStore {
    rows: Mutex<HashMap<Uuid, TradeCommand>>,
}

impl InMemoryCommandStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every row, ordered by creation time then id (diagnostics and tests).
    pub fn all(&self) -> Vec<TradeCommand> {
        let mut rows: Vec<TradeCommand> = self
            .rows
            .lock()
            .expect("command store lock")
            .values()
            .cloned()
            .collect();
        rows.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.command_id.cmp(&b.command_id))
        });
        rows
    }

    /// Rows of one chat, oldest first.
    pub fn for_chat(&self, organization_id: &str, chat_id: i64) -> Vec<TradeCommand> {
        self.all()
            .into_iter()
            .filter(|c| c.organization_id == organization_id && c.chat_id == chat_id)
            .collect()
    }
}

#[async_trait]
impl CommandStore for InMemoryCommandStore {
    async fn insert(&self, command: &TradeCommand) -> Result<(), StoreError> {
        let mut rows = self.rows.lock().expect("command store lock");
        if rows.contains_key(&command.command_id) {
            return Err(StoreError::Conflict("command id already exists".into()));
        }
        rows.insert(command.command_id, command.clone());
        Ok(())
    }

    async fn get(&self, command_id: Uuid) -> Result<Option<TradeCommand>, StoreError> {
        Ok(self
            .rows
            .lock()
            .expect("command store lock")
            .get(&command_id)
            .cloned())
    }

    async fn find_by_nonce(
        &self,
        organization_id: &str,
        chat_id: i64,
        nonce_sha256: &[u8; 32],
    ) -> Result<Option<TradeCommand>, StoreError> {
        let rows = self.rows.lock().expect("command store lock");
        Ok(rows
            .values()
            .filter(|c| {
                c.organization_id == organization_id
                    && c.chat_id == chat_id
                    && &c.nonce_sha256 == nonce_sha256
            })
            .max_by_key(|c| c.created_at)
            .cloned())
    }

    async fn transition(
        &self,
        command_id: Uuid,
        from: CommandStatus,
        to: CommandStatus,
        now: DateTime<Utc>,
        failure_code: Option<&str>,
    ) -> Result<bool, StoreError> {
        let mut rows = self.rows.lock().expect("command store lock");
        let Some(row) = rows.get_mut(&command_id) else {
            return Ok(false);
        };
        if row.status != from {
            return Ok(false);
        }
        row.status = to;
        row.updated_at = now;
        row.failure_code = match to {
            CommandStatus::Failed => failure_code.map(str::to_string),
            _ => None,
        };
        Ok(true)
    }

    async fn pending(
        &self,
        organization_id: &str,
        chat_id: i64,
        now: DateTime<Utc>,
    ) -> Result<Vec<TradeCommand>, StoreError> {
        let rows = self.rows.lock().expect("command store lock");
        let mut pending: Vec<TradeCommand> = rows
            .values()
            .filter(|c| {
                c.organization_id == organization_id
                    && c.chat_id == chat_id
                    && c.status == CommandStatus::Pending
                    && !c.is_expired(now)
            })
            .cloned()
            .collect();
        pending.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.command_id.cmp(&b.command_id))
        });
        Ok(pending)
    }

    async fn cancel_pending(
        &self,
        organization_id: &str,
        chat_id: i64,
        now: DateTime<Utc>,
    ) -> Result<Vec<Uuid>, StoreError> {
        let mut rows = self.rows.lock().expect("command store lock");
        let mut cancelled = Vec::new();
        for row in rows.values_mut() {
            if row.organization_id == organization_id
                && row.chat_id == chat_id
                && row.status == CommandStatus::Pending
            {
                row.status = CommandStatus::Cancelled;
                row.updated_at = now;
                cancelled.push(row.command_id);
            }
        }
        cancelled.sort();
        Ok(cancelled)
    }

    async fn expire_overdue(&self, now: DateTime<Utc>) -> Result<Vec<Uuid>, StoreError> {
        let mut rows = self.rows.lock().expect("command store lock");
        let mut expired = Vec::new();
        for row in rows.values_mut() {
            if row.status == CommandStatus::Pending && row.is_expired(now) {
                row.status = CommandStatus::Expired;
                row.updated_at = now;
                expired.push(row.command_id);
            }
        }
        expired.sort();
        Ok(expired)
    }

    async fn spent_since(
        &self,
        organization_id: &str,
        chat_id: i64,
        since: DateTime<Utc>,
    ) -> Result<u64, StoreError> {
        let rows = self.rows.lock().expect("command store lock");
        let mut total: u64 = 0;
        for row in rows.values() {
            if row.organization_id == organization_id
                && row.chat_id == chat_id
                && row.action == TradeAction::Buy
                && consumes_capacity(row.status)
                && row.created_at >= since
            {
                total = total.saturating_add(row.amount_lamports_or_zero());
            }
        }
        Ok(total)
    }

    async fn count_since(
        &self,
        organization_id: &str,
        chat_id: i64,
        since: DateTime<Utc>,
    ) -> Result<u64, StoreError> {
        let rows = self.rows.lock().expect("command store lock");
        Ok(rows
            .values()
            .filter(|c| {
                c.organization_id == organization_id
                    && c.chat_id == chat_id
                    && c.created_at >= since
            })
            .count() as u64)
    }
}

/// A store that always fails — proves the flow fails closed on a database
/// outage.
#[derive(Debug, Default)]
pub struct BrokenCommandStore {
    /// Detail surfaced on every call.
    pub detail: String,
}

impl BrokenCommandStore {
    /// Build one with a fixed detail string.
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }

    fn err(&self) -> StoreError {
        StoreError::Unavailable(if self.detail.is_empty() {
            "command store unavailable".to_string()
        } else {
            self.detail.clone()
        })
    }
}

#[async_trait]
impl CommandStore for BrokenCommandStore {
    async fn insert(&self, _command: &TradeCommand) -> Result<(), StoreError> {
        Err(self.err())
    }

    async fn get(&self, _command_id: Uuid) -> Result<Option<TradeCommand>, StoreError> {
        Err(self.err())
    }

    async fn find_by_nonce(
        &self,
        _organization_id: &str,
        _chat_id: i64,
        _nonce_sha256: &[u8; 32],
    ) -> Result<Option<TradeCommand>, StoreError> {
        Err(self.err())
    }

    async fn transition(
        &self,
        _command_id: Uuid,
        _from: CommandStatus,
        _to: CommandStatus,
        _now: DateTime<Utc>,
        _failure_code: Option<&str>,
    ) -> Result<bool, StoreError> {
        Err(self.err())
    }

    async fn pending(
        &self,
        _organization_id: &str,
        _chat_id: i64,
        _now: DateTime<Utc>,
    ) -> Result<Vec<TradeCommand>, StoreError> {
        Err(self.err())
    }

    async fn cancel_pending(
        &self,
        _organization_id: &str,
        _chat_id: i64,
        _now: DateTime<Utc>,
    ) -> Result<Vec<Uuid>, StoreError> {
        Err(self.err())
    }

    async fn expire_overdue(&self, _now: DateTime<Utc>) -> Result<Vec<Uuid>, StoreError> {
        Err(self.err())
    }

    async fn spent_since(
        &self,
        _organization_id: &str,
        _chat_id: i64,
        _since: DateTime<Utc>,
    ) -> Result<u64, StoreError> {
        Err(self.err())
    }

    async fn count_since(
        &self,
        _organization_id: &str,
        _chat_id: i64,
        _since: DateTime<Utc>,
    ) -> Result<u64, StoreError> {
        Err(self.err())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::nonce;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    fn spec(action: TradeAction, amount: Option<u64>) -> PendingSpec {
        PendingSpec {
            organization_id: "org-1".into(),
            user_id: 42,
            chat_id: 100,
            action,
            mint: "So11111111111111111111111111111111111111112".into(),
            amount_lamports: amount,
            percent_bps: match action {
                TradeAction::Buy => None,
                TradeAction::Sell => Some(10_000),
            },
            slippage_bps: 100,
        }
    }

    #[test]
    fn a_confirmed_command_is_only_replayed_after_the_grace_window() {
        let (command, _) = TradeCommand::open(spec(TradeAction::Buy, Some(1)), at(0));
        assert!(
            !is_confirm_retryable(&command, at(1)),
            "a pending row is not a replay"
        );
        let mut confirmed = command.clone();
        confirmed.status = CommandStatus::Confirmed;
        confirmed.updated_at = at(10);
        assert!(!is_confirm_retryable(&confirmed, at(10)));
        assert!(!is_confirm_retryable(
            &confirmed,
            at(10 + CONFIRM_GRACE_SECS - 1)
        ));
        assert!(is_confirm_retryable(
            &confirmed,
            at(10 + CONFIRM_GRACE_SECS)
        ));
        // Terminal states are final, whatever their age.
        for status in [
            CommandStatus::Executed,
            CommandStatus::Failed,
            CommandStatus::Expired,
            CommandStatus::Cancelled,
        ] {
            let mut done = command.clone();
            done.status = status;
            assert!(!is_confirm_retryable(&done, at(10_000)));
        }
    }

    #[tokio::test]
    async fn open_binds_parameters_and_stores_only_the_digest() {
        let (row, nonce) = TradeCommand::open(spec(TradeAction::Buy, Some(50_000_000)), at(1_000));
        assert_eq!(row.status, CommandStatus::Pending);
        assert_eq!(row.idempotency_key, row.command_id);
        assert_eq!(row.expires_at, at(1_060));
        assert_eq!(row.nonce_sha256, nonce.sha256());
        assert!(nonce::verify(nonce.expose(), &row.nonce_sha256));
        assert_eq!(row.amount_lamports_or_zero(), 50_000_000);
        assert_eq!(row.percent_bps_or_zero(), 0);
        assert!(!row.is_expired(at(1_059)));
        assert!(row.is_expired(at(1_060)));

        let store = InMemoryCommandStore::new();
        store.insert(&row).await.unwrap();
        let found = store
            .find_by_nonce("org-1", 100, &nonce.sha256())
            .await
            .unwrap()
            .expect("found");
        assert_eq!(found, row);
        // Wrong chat, wrong org and wrong digest all miss.
        assert!(store
            .find_by_nonce("org-1", 101, &nonce.sha256())
            .await
            .unwrap()
            .is_none());
        assert!(store
            .find_by_nonce("org-2", 100, &nonce.sha256())
            .await
            .unwrap()
            .is_none());
        assert!(store
            .find_by_nonce("org-1", 100, &[0u8; 32])
            .await
            .unwrap()
            .is_none());
    }

    #[test]
    fn machine_allows_only_the_documented_edges() {
        let (row, _n) = TradeCommand::open(spec(TradeAction::Buy, Some(1)), at(1_000));
        assert!(transition_confirm(&row, at(1_001)).is_ok());
        assert!(transition_cancel(&row, at(1_001)).is_ok());
        assert_eq!(
            transition_confirm(&row, at(1_060)),
            Err(TransitionError::Expired)
        );
        assert!(transition_expired(&row, at(1_060)).is_ok());
        assert!(matches!(
            transition_expired(&row, at(1_010)),
            Err(TransitionError::Illegal { .. })
        ));
        assert!(matches!(
            transition_executed(&row, at(1_001)),
            Err(TransitionError::Illegal { .. })
        ));

        let mut confirmed = row.clone();
        confirmed.status = CommandStatus::Confirmed;
        assert!(transition_executed(&confirmed, at(1_001)).is_ok());
        assert!(transition_failed(&confirmed, at(1_001)).is_ok());
        assert_eq!(
            transition_confirm(&confirmed, at(1_001)),
            Err(TransitionError::Illegal {
                from: CommandStatus::Confirmed,
                to: CommandStatus::Confirmed
            })
        );

        let mut executed = confirmed.clone();
        executed.status = CommandStatus::Executed;
        assert_eq!(
            transition_confirm(&executed, at(1_001)),
            Err(TransitionError::AlreadyResolved(CommandStatus::Executed))
        );
        assert_eq!(
            transition_cancel(&executed, at(1_001)),
            Err(TransitionError::AlreadyResolved(CommandStatus::Executed))
        );
        assert_eq!(
            transition_executed(&executed, at(1_001)),
            Err(TransitionError::AlreadyResolved(CommandStatus::Executed))
        );
        assert!(executed.status.is_terminal());
        assert!(!CommandStatus::Pending.is_terminal());
        assert!(!CommandStatus::Confirmed.is_terminal());
        assert_eq!(TransitionError::Expired.code(), "expired");
        assert_eq!(
            TransitionError::AlreadyResolved(CommandStatus::Expired).code(),
            "already_resolved"
        );
        assert_eq!(
            TransitionError::Illegal {
                from: CommandStatus::Pending,
                to: CommandStatus::Executed
            }
            .code(),
            "illegal_transition"
        );
    }

    #[test]
    fn status_labels_round_trip() {
        for status in [
            CommandStatus::Pending,
            CommandStatus::Confirmed,
            CommandStatus::Executed,
            CommandStatus::Failed,
            CommandStatus::Expired,
            CommandStatus::Cancelled,
            CommandStatus::Denied,
        ] {
            assert_eq!(CommandStatus::from_label(status.as_str()), Some(status));
        }
        assert_eq!(CommandStatus::from_label("PENDING"), None);
        assert_eq!(CommandStatus::from_label(""), None);
    }

    #[tokio::test]
    async fn transitions_are_compare_and_set() {
        let (row, _n) = TradeCommand::open(spec(TradeAction::Buy, Some(1)), at(1_000));
        let store = InMemoryCommandStore::new();
        store.insert(&row).await.unwrap();
        assert!(store
            .transition(
                row.command_id,
                CommandStatus::Pending,
                CommandStatus::Confirmed,
                at(1_001),
                None
            )
            .await
            .unwrap());
        // A second confirmation from the same source state loses the race.
        assert!(!store
            .transition(
                row.command_id,
                CommandStatus::Pending,
                CommandStatus::Confirmed,
                at(1_002),
                None
            )
            .await
            .unwrap());
        assert!(store
            .transition(
                row.command_id,
                CommandStatus::Confirmed,
                CommandStatus::Failed,
                at(1_003),
                Some("risk_rejected")
            )
            .await
            .unwrap());
        let row = store.get(row.command_id).await.unwrap().unwrap();
        assert_eq!(row.status, CommandStatus::Failed);
        assert_eq!(row.failure_code.as_deref(), Some("risk_rejected"));
        assert_eq!(row.updated_at, at(1_003));
        // Unknown id is not an error, just a lost CAS.
        assert!(!store
            .transition(
                Uuid::new_v4(),
                CommandStatus::Pending,
                CommandStatus::Confirmed,
                at(1_004),
                None
            )
            .await
            .unwrap());
        assert_eq!(
            store.insert(&row).await.expect_err("duplicate id").code(),
            "store_conflict"
        );
    }

    #[tokio::test]
    async fn pending_cancel_expire_and_counters() {
        let store = InMemoryCommandStore::new();
        let (a, _) = TradeCommand::open(spec(TradeAction::Buy, Some(10)), at(1_000));
        let (b, _) = TradeCommand::open(spec(TradeAction::Buy, Some(20)), at(1_001));
        let (c, _) = TradeCommand::open(spec(TradeAction::Sell, None), at(1_002));
        for row in [&a, &b, &c] {
            store.insert(row).await.unwrap();
        }
        assert_eq!(
            store.pending("org-1", 100, at(1_010)).await.unwrap().len(),
            3
        );
        assert_eq!(store.count_since("org-1", 100, at(1_000)).await.unwrap(), 3);
        assert_eq!(store.count_since("org-1", 100, at(1_002)).await.unwrap(), 1);
        assert_eq!(store.count_since("org-2", 100, at(1_000)).await.unwrap(), 0);

        // Nothing is spent until the port is called (confirmed/executed).
        assert_eq!(store.spent_since("org-1", 100, at(0)).await.unwrap(), 0);
        store
            .transition(
                a.command_id,
                CommandStatus::Pending,
                CommandStatus::Confirmed,
                at(1_005),
                None,
            )
            .await
            .unwrap();
        store
            .transition(
                a.command_id,
                CommandStatus::Confirmed,
                CommandStatus::Executed,
                at(1_006),
                None,
            )
            .await
            .unwrap();
        assert_eq!(store.spent_since("org-1", 100, at(0)).await.unwrap(), 10);
        // Sells never consume the SOL cap.
        store
            .transition(
                c.command_id,
                CommandStatus::Pending,
                CommandStatus::Confirmed,
                at(1_007),
                None,
            )
            .await
            .unwrap();
        assert_eq!(store.spent_since("org-1", 100, at(0)).await.unwrap(), 10);

        // TTL housekeeping expires only the overdue, unconfirmed row.
        let expired = store.expire_overdue(at(2_000)).await.unwrap();
        assert_eq!(expired, vec![b.command_id]);
        assert!(store
            .pending("org-1", 100, at(2_000))
            .await
            .unwrap()
            .is_empty());

        // Cancellation touches only pending rows.
        let (d, _) = TradeCommand::open(spec(TradeAction::Buy, Some(5)), at(2_001));
        store.insert(&d).await.unwrap();
        let cancelled = store.cancel_pending("org-1", 100, at(2_002)).await.unwrap();
        assert_eq!(cancelled, vec![d.command_id]);
        assert!(store
            .cancel_pending("org-1", 100, at(2_003))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn broken_store_never_silently_succeeds() {
        let store = BrokenCommandStore::new("db down");
        let (row, n) = TradeCommand::open(spec(TradeAction::Buy, Some(1)), at(1_000));
        assert!(store.insert(&row).await.is_err());
        assert!(store.get(row.command_id).await.is_err());
        assert!(store
            .find_by_nonce("org-1", 100, &n.sha256())
            .await
            .is_err());
        assert!(store.pending("org-1", 100, at(1_000)).await.is_err());
        assert!(store.cancel_pending("org-1", 100, at(1_000)).await.is_err());
        assert!(store.expire_overdue(at(1_000)).await.is_err());
        assert!(store.spent_since("org-1", 100, at(0)).await.is_err());
        assert!(store.count_since("org-1", 100, at(0)).await.is_err());
        assert!(store
            .transition(
                row.command_id,
                CommandStatus::Pending,
                CommandStatus::Cancelled,
                at(1_001),
                None
            )
            .await
            .is_err());
        assert_eq!(
            BrokenCommandStore::default()
                .expire_overdue(at(0))
                .await
                .unwrap_err()
                .detail(),
            "command store unavailable"
        );
    }
}
