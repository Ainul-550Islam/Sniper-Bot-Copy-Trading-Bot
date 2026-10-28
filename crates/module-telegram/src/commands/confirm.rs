//! `/confirm <nonce>`, `/cancel <nonce>` and `/pending`.
//!
//! This is where a pending command becomes an executed one — and the only
//! place in the module that calls
//! [`crate::trade_port::TradeCommandPort::place`]. The order of operations is
//! fixed and each step fails closed:
//!
//! 1. resolve the chat link (tenant + linked user);
//! 2. housekeeping: overdue pending commands become `expired` (audited);
//! 3. find the command by **nonce digest** inside this `(organization, chat)`;
//! 4. check the requester is the one who asked for it;
//! 5. gate the state machine (TTL, already-resolved, single use);
//! 6. compare-and-set `pending → confirmed` — the race is lost here, never
//!    after money moved;
//! 7. re-check the kill switch (it may have been engaged since the request);
//! 8. call the port with `idempotency_key = command_id`;
//! 9. compare-and-set `confirmed → executed|failed` and audit.

use chrono::{DateTime, Utc};

use crate::commands::audit::{AuditAmount, AuditBps, AuditDetail, AuditEvent};
use crate::commands::nonce;
use crate::commands::pending::{
    self, CommandStatus, TradeCommand, TransitionError, COMMAND_TTL_SECS,
};
use crate::commands::reply::{self, Reply};
use crate::commands::TradeConsole;
use crate::trade_port::OrderRequest;

impl TradeConsole {
    /// Handle `/confirm <nonce>`.
    pub(crate) async fn handle_confirm(
        &self,
        chat_id: i64,
        user_id: Option<i64>,
        nonce_input: &str,
        now: DateTime<Utc>,
    ) -> Reply {
        let Some(user_id) = user_id else {
            return self
                .deny_unlinked(chat_id, "unlinked_chat", "this message has no sender", now)
                .await;
        };
        let link = match self.resolve_link(chat_id, user_id).await {
            Ok(link) => link,
            Err(deny) => return self.deny_link(chat_id, None, deny, now).await,
        };
        let org = link.organization_id.clone();
        self.expire_overdue(now).await;

        let Some(candidate) = nonce::Nonce::from_input(nonce_input) else {
            return self
                .deny(
                    Some(&org),
                    chat_id,
                    "malformed_nonce",
                    "a confirmation nonce is 8+ letters or digits",
                    now,
                )
                .await;
        };
        let row = match self
            .command_store
            .find_by_nonce(&org, chat_id, &candidate.sha256())
            .await
        {
            Ok(Some(row)) => row,
            Ok(None) => {
                return self
                    .deny(
                        Some(&org),
                        chat_id,
                        "unknown_nonce",
                        "no pending command matches that nonce in this chat",
                        now,
                    )
                    .await
            }
            Err(err) => {
                return self
                    .deny(
                        Some(&org),
                        chat_id,
                        "command_store_unavailable",
                        err.detail(),
                        now,
                    )
                    .await
            }
        };
        // Constant-time re-verification against the stored digest, in addition
        // to the store's lookup: a store that matched loosely must not be able
        // to release a trade.
        if !nonce::verify(candidate.expose(), &row.nonce_sha256) {
            return self
                .deny(
                    Some(&org),
                    chat_id,
                    "nonce_mismatch",
                    "the nonce does not match the stored digest",
                    now,
                )
                .await;
        }
        if row.user_id != user_id {
            return self
                .deny(
                    Some(&org),
                    chat_id,
                    "wrong_user",
                    "only the user who requested this command can confirm it",
                    now,
                )
                .await;
        }

        // Already executed: answer, never execute again.
        if row.status == CommandStatus::Executed {
            let _ = self
                .audit(
                    Some(row.command_id),
                    Some(&org),
                    chat_id,
                    AuditEvent::Denied,
                    AuditDetail::new()
                        .reason("already_executed")
                        .action(row.action),
                    now,
                )
                .await;
            return reply::already_executed(row.action);
        }
        if row.status.is_terminal() {
            let _ = self
                .audit(
                    Some(row.command_id),
                    Some(&org),
                    chat_id,
                    AuditEvent::Denied,
                    AuditDetail::new()
                        .reason("already_resolved")
                        .status(row.status.as_str())
                        .action(row.action),
                    now,
                )
                .await;
            return reply::already_resolved(row.status.as_str());
        }

        // A row that is already `confirmed` is only replayed once the grace
        // window has passed (see `pending::CONFIRM_GRACE_SECS`): inside it the
        // first confirmation is still in flight, so a second tap must be told
        // to wait rather than firing the port again. The replay reuses
        // `idempotency_key`, so even then the order cannot execute twice.
        if row.status == CommandStatus::Confirmed && !pending::is_confirm_retryable(&row, now) {
            let _ = self
                .audit(
                    Some(row.command_id),
                    Some(&org),
                    chat_id,
                    AuditEvent::Denied,
                    AuditDetail::new()
                        .reason("already_confirmed")
                        .action(row.action)
                        .status(row.status.as_str()),
                    now,
                )
                .await;
            return reply::confirm_in_flight(row.action);
        }
        let already_confirmed = row.status == CommandStatus::Confirmed;
        if !already_confirmed {
            match pending::transition_confirm(&row, now) {
                Ok(()) => {}
                Err(TransitionError::Expired) => {
                    let _ = self
                        .command_store
                        .transition(
                            row.command_id,
                            CommandStatus::Pending,
                            CommandStatus::Expired,
                            now,
                            None,
                        )
                        .await;
                    let _ = self
                        .audit(
                            Some(row.command_id),
                            Some(&org),
                            chat_id,
                            AuditEvent::Expired,
                            AuditDetail::new().action(row.action),
                            now,
                        )
                        .await;
                    return reply::expired(COMMAND_TTL_SECS);
                }
                Err(err) => {
                    let _ = self
                        .audit(
                            Some(row.command_id),
                            Some(&org),
                            chat_id,
                            AuditEvent::Denied,
                            AuditDetail::new()
                                .reason(err.code())
                                .status(row.status.as_str())
                                .action(row.action),
                            now,
                        )
                        .await;
                    return reply::already_resolved(row.status.as_str());
                }
            }
            match self
                .command_store
                .transition(
                    row.command_id,
                    CommandStatus::Pending,
                    CommandStatus::Confirmed,
                    now,
                    None,
                )
                .await
            {
                Ok(true) => {}
                Ok(false) => {
                    // Another confirmation (or a cancellation) won the race.
                    let current = self
                        .command_store
                        .get(row.command_id)
                        .await
                        .ok()
                        .flatten()
                        .map(|c| c.status)
                        .unwrap_or(CommandStatus::Cancelled);
                    let _ = self
                        .audit(
                            Some(row.command_id),
                            Some(&org),
                            chat_id,
                            AuditEvent::Denied,
                            AuditDetail::new()
                                .reason("lost_race")
                                .status(current.as_str())
                                .action(row.action),
                            now,
                        )
                        .await;
                    return reply::already_resolved(current.as_str());
                }
                Err(err) => {
                    return self
                        .deny(
                            Some(&org),
                            chat_id,
                            "command_store_unavailable",
                            err.detail(),
                            now,
                        )
                        .await
                }
            }
            let _ = self
                .audit(
                    Some(row.command_id),
                    Some(&org),
                    chat_id,
                    AuditEvent::Confirmed,
                    AuditDetail::new()
                        .action(row.action)
                        .mint(&row.mint)
                        .mode("confirmed"),
                    now,
                )
                .await;
        }

        // The kill switch can be engaged between the request and the tap.
        if self.state.kill_switch() || self.port.is_halted().await {
            return self
                .fail_command(
                    &row,
                    &org,
                    chat_id,
                    "halted",
                    "the kill switch is engaged",
                    now,
                )
                .await;
        }

        let order = OrderRequest {
            command_id: row.command_id,
            idempotency_key: row.idempotency_key,
            organization_id: org.clone(),
            chat_id: row.chat_id,
            user_id: row.user_id,
            action: row.action,
            mint: row.mint.clone(),
            amount_lamports: row.amount_lamports,
            percent_bps: row.percent_bps,
            slippage_bps: row.slippage_bps,
        };
        match self.port.place(&order).await {
            Ok(receipt) => {
                let updated = self
                    .command_store
                    .transition(
                        row.command_id,
                        CommandStatus::Confirmed,
                        CommandStatus::Executed,
                        now,
                        None,
                    )
                    .await
                    .unwrap_or(false);
                if !updated {
                    // The command was already final: the port is idempotent on
                    // `idempotency_key`, so nothing ran twice.
                    return reply::already_executed(row.action);
                }
                let _ = self
                    .audit(
                        Some(row.command_id),
                        Some(&org),
                        chat_id,
                        AuditEvent::Executed,
                        AuditDetail::new()
                            .action(row.action)
                            .mint(&row.mint)
                            .lamports(AuditAmount::AmountLamports, row.amount_lamports_or_zero())
                            .bps(AuditBps::PercentBps, row.percent_bps_or_zero())
                            .bps(AuditBps::SlippageBps, row.slippage_bps)
                            .status("executed"),
                        now,
                    )
                    .await;
                reply::executed(row.action, &receipt.reference, &receipt.detail)
            }
            Err(err) => {
                self.fail_command(&row, &org, chat_id, err.code(), err.detail(), now)
                    .await
            }
        }
    }

    /// Handle `/cancel <nonce>`.
    pub(crate) async fn handle_cancel(
        &self,
        chat_id: i64,
        user_id: Option<i64>,
        nonce_input: &str,
        now: DateTime<Utc>,
    ) -> Reply {
        let Some(user_id) = user_id else {
            return self
                .deny_unlinked(chat_id, "unlinked_chat", "this message has no sender", now)
                .await;
        };
        let link = match self.resolve_link(chat_id, user_id).await {
            Ok(link) => link,
            Err(deny) => return self.deny_link(chat_id, None, deny, now).await,
        };
        let org = link.organization_id.clone();
        self.expire_overdue(now).await;

        let Some(candidate) = nonce::Nonce::from_input(nonce_input) else {
            return self
                .deny(
                    Some(&org),
                    chat_id,
                    "malformed_nonce",
                    "a confirmation nonce is 8+ letters or digits",
                    now,
                )
                .await;
        };
        let row = match self
            .command_store
            .find_by_nonce(&org, chat_id, &candidate.sha256())
            .await
        {
            Ok(Some(row)) => row,
            Ok(None) => {
                return self
                    .deny(
                        Some(&org),
                        chat_id,
                        "unknown_nonce",
                        "no pending command matches that nonce in this chat",
                        now,
                    )
                    .await
            }
            Err(err) => {
                return self
                    .deny(
                        Some(&org),
                        chat_id,
                        "command_store_unavailable",
                        err.detail(),
                        now,
                    )
                    .await
            }
        };
        if row.user_id != user_id {
            return self
                .deny(
                    Some(&org),
                    chat_id,
                    "wrong_user",
                    "only the user who requested this command can cancel it",
                    now,
                )
                .await;
        }
        match pending::transition_cancel(&row, now) {
            Ok(()) => {}
            Err(TransitionError::Expired) => {
                let _ = self
                    .command_store
                    .transition(
                        row.command_id,
                        CommandStatus::Pending,
                        CommandStatus::Expired,
                        now,
                        None,
                    )
                    .await;
                let _ = self
                    .audit(
                        Some(row.command_id),
                        Some(&org),
                        chat_id,
                        AuditEvent::Expired,
                        AuditDetail::new().action(row.action),
                        now,
                    )
                    .await;
                return reply::expired(COMMAND_TTL_SECS);
            }
            Err(err) => {
                let _ = self
                    .audit(
                        Some(row.command_id),
                        Some(&org),
                        chat_id,
                        AuditEvent::Denied,
                        AuditDetail::new()
                            .reason(err.code())
                            .status(row.status.as_str())
                            .action(row.action),
                        now,
                    )
                    .await;
                return reply::already_resolved(row.status.as_str());
            }
        }
        match self
            .command_store
            .transition(
                row.command_id,
                CommandStatus::Pending,
                CommandStatus::Cancelled,
                now,
                None,
            )
            .await
        {
            Ok(true) => {
                let _ = self
                    .audit(
                        Some(row.command_id),
                        Some(&org),
                        chat_id,
                        AuditEvent::Cancelled,
                        AuditDetail::new().action(row.action).mint(&row.mint),
                        now,
                    )
                    .await;
                reply::cancelled()
            }
            Ok(false) => {
                let current = self
                    .command_store
                    .get(row.command_id)
                    .await
                    .ok()
                    .flatten()
                    .map(|c| c.status)
                    .unwrap_or(CommandStatus::Cancelled);
                reply::already_resolved(current.as_str())
            }
            Err(err) => {
                self.deny(
                    Some(&org),
                    chat_id,
                    "command_store_unavailable",
                    err.detail(),
                    now,
                )
                .await
            }
        }
    }

    /// Handle `/pending`.
    pub(crate) async fn handle_pending(
        &self,
        chat_id: i64,
        user_id: Option<i64>,
        now: DateTime<Utc>,
    ) -> Reply {
        let Some(user_id) = user_id else {
            return self
                .deny_unlinked(chat_id, "unlinked_chat", "this message has no sender", now)
                .await;
        };
        let link = match self.resolve_link(chat_id, user_id).await {
            Ok(link) => link,
            Err(deny) => return self.deny_link(chat_id, None, deny, now).await,
        };
        let org = link.organization_id.clone();
        self.expire_overdue(now).await;
        let rows = match self.command_store.pending(&org, chat_id, now).await {
            Ok(rows) => rows,
            Err(err) => {
                return self
                    .deny(
                        Some(&org),
                        chat_id,
                        "command_store_unavailable",
                        err.detail(),
                        now,
                    )
                    .await
            }
        };
        if rows.is_empty() {
            return reply::pending_empty();
        }
        let lines: Vec<String> = rows
            .iter()
            .map(|row| {
                reply::pending_line(
                    row.action,
                    &row.mint,
                    row.amount_lamports,
                    row.percent_bps,
                    (row.expires_at - now).num_seconds().max(0),
                    &short_id(row.command_id),
                )
            })
            .collect();
        // For `/pending` the timestamps are relative to now, which the reply
        // builder renders verbatim.
        reply::pending_list(&lines)
    }

    /// Move overdue commands to `expired` and audit each transition.
    async fn expire_overdue(&self, now: DateTime<Utc>) {
        let Ok(ids) = self.command_store.expire_overdue(now).await else {
            // Housekeeping failures must never block a command: the row's own
            // TTL check still refuses a stale nonce.
            return;
        };
        for command_id in ids {
            let Ok(Some(row)) = self.command_store.get(command_id).await else {
                continue;
            };
            let _ = self
                .audit(
                    Some(command_id),
                    Some(&row.organization_id),
                    row.chat_id,
                    AuditEvent::Expired,
                    AuditDetail::new()
                        .action(row.action)
                        .status(row.status.as_str()),
                    now,
                )
                .await;
        }
    }

    /// Mark a confirmed command failed, audit it, and answer the chat.
    async fn fail_command(
        &self,
        row: &TradeCommand,
        organization_id: &str,
        chat_id: i64,
        code: &str,
        detail: &str,
        now: DateTime<Utc>,
    ) -> Reply {
        let _ = self
            .command_store
            .transition(
                row.command_id,
                CommandStatus::Confirmed,
                CommandStatus::Failed,
                now,
                Some(code),
            )
            .await;
        let _ = self
            .audit(
                Some(row.command_id),
                Some(organization_id),
                chat_id,
                AuditEvent::Failed,
                AuditDetail::new()
                    .reason(code)
                    .action(row.action)
                    .mint(&row.mint)
                    .lamports(AuditAmount::AmountLamports, row.amount_lamports_or_zero())
                    .status("failed")
                    .failure_detail(detail),
                now,
            )
            .await;
        reply::execution_failed(row.action, code, detail)
    }
}

/// The first 8 characters of a command id, for the `/pending` listing (the
/// nonce is not stored in a form that can be shown, by design).
pub fn short_id(command_id: uuid::Uuid) -> String {
    command_id.to_string().chars().take(8).collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_ids_are_stable_and_truncated() {
        let id = uuid::Uuid::from_u128(0x1234_5678_9abc_def0_1234_5678_9abc_def0);
        assert_eq!(short_id(id), "12345678");
        assert_eq!(short_id(id).len(), 8);
    }

    #[test]
    fn ttl_constant_is_one_minute() {
        assert_eq!(COMMAND_TTL_SECS, 60);
    }
}
