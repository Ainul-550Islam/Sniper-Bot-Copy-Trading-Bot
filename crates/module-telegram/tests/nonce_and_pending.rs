//! BATCH 04 — nonce handling and the pending-command state machine.
//!
//! Covers: CSPRNG nonce shape, hashing/constant-time verification, single use,
//! TTL expiry, chat/user binding, duplicate confirmation idempotency, cancel,
//! and "a consent revocation cancels pending commands".

use chrono::{DateTime, Utc};
use uuid::Uuid;

use module_telegram::commands::nonce::{self, Nonce};
use module_telegram::commands::pending::{
    transition_cancel, transition_confirm, transition_execute_or_fail, transition_expired,
    CommandStatus, CommandStore, InMemoryCommandStore, PendingSpec, TradeCommand, COMMAND_TTL_SECS,
};
use module_telegram::TradeAction;

fn at(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(secs, 0).unwrap()
}

fn spec(action: TradeAction) -> PendingSpec {
    PendingSpec {
        organization_id: "org-1".into(),
        user_id: 111,
        chat_id: 100,
        action,
        mint: "So11111111111111111111111111111111111111112".into(),
        amount_lamports: match action {
            TradeAction::Buy => Some(25_000_000),
            TradeAction::Sell => None,
        },
        percent_bps: match action {
            TradeAction::Buy => None,
            TradeAction::Sell => Some(10_000),
        },
        slippage_bps: 100,
    }
}

#[test]
fn nonces_are_long_random_and_never_printed() {
    let mut seen = std::collections::HashSet::new();
    for _ in 0..512 {
        let nonce = Nonce::generate();
        let text = nonce.expose().to_string();
        assert!(text.len() >= 8, "spec floor is 8 chars: {text}");
        assert!(text.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(seen.insert(text));
        assert_eq!(format!("{nonce:?}"), "Nonce(<redacted>)");
        assert_eq!(nonce.sha256().len(), 32);
    }
}

#[test]
fn only_the_digest_matches() {
    let nonce = Nonce::generate();
    let digest = nonce.sha256();
    assert!(nonce::verify(nonce.expose(), &digest));
    assert!(!nonce::verify("0000000000000000", &digest));
    assert!(!nonce::verify("", &digest));
    assert!(!nonce::verify("too-short", &digest));
    assert_eq!(nonce::hash("abc"), nonce::hash("abc"));
    assert_ne!(nonce::hash("abc"), nonce::hash("abd"));
}

#[tokio::test]
async fn a_pending_command_stores_no_plaintext_nonce() {
    let store = InMemoryCommandStore::new();
    let (row, nonce) = TradeCommand::open(spec(TradeAction::Buy), at(10_000));
    store.insert(&row).await.unwrap();

    // Nothing in the row (as a struct) equals the plaintext.
    let rendered = format!("{:?}{:?}", row, store.all());
    assert!(!rendered.contains(nonce.expose()), "plaintext leaked");
    assert_eq!(row.nonce_sha256, nonce.sha256());
    assert_eq!(row.expires_at, at(10_000 + COMMAND_TTL_SECS));
    assert_eq!(row.idempotency_key, row.command_id);

    // Lookup works by digest, scoped to (org, chat).
    let found = store
        .find_by_nonce("org-1", 100, &nonce::hash(nonce.expose()))
        .await
        .unwrap()
        .expect("found by digest");
    assert_eq!(found.command_id, row.command_id);
    assert!(store
        .find_by_nonce("org-2", 100, &nonce.sha256())
        .await
        .unwrap()
        .is_none());
    assert!(store
        .find_by_nonce("org-1", 101, &nonce.sha256())
        .await
        .unwrap()
        .is_none());
}

#[test]
fn ttl_expiry_is_enforced_by_the_machine() {
    let (row, _n) = TradeCommand::open(spec(TradeAction::Buy), at(10_000));
    assert!(transition_confirm(&row, at(10_059)).is_ok());
    assert_eq!(
        transition_confirm(&row, at(10_060)),
        Err(module_telegram::commands::pending::TransitionError::Expired)
    );
    assert!(transition_expired(&row, at(10_060)).is_ok());
    assert!(transition_expired(&row, at(10_059)).is_err());
}

#[test]
fn single_use_and_duplicate_confirmation_is_idempotent() {
    let (row, _n) = TradeCommand::open(spec(TradeAction::Buy), at(10_000));
    // Pending can only be confirmed once (the CAS in the store decides).
    assert!(transition_confirm(&row, at(10_001)).is_ok());

    let mut confirmed = row.clone();
    confirmed.status = CommandStatus::Confirmed;
    assert!(transition_execute_or_fail(&confirmed, CommandStatus::Executed).is_ok());
    assert!(transition_execute_or_fail(&confirmed, CommandStatus::Failed).is_ok());

    let mut executed = confirmed.clone();
    executed.status = CommandStatus::Executed;
    // A second confirmation is answered, never applied twice.
    assert_eq!(
        transition_confirm(&executed, at(10_002)),
        Err(
            module_telegram::commands::pending::TransitionError::AlreadyResolved(
                CommandStatus::Executed
            )
        )
    );
    assert_eq!(
        transition_cancel(&executed, at(10_002)),
        Err(
            module_telegram::commands::pending::TransitionError::AlreadyResolved(
                CommandStatus::Executed
            )
        )
    );
}

#[tokio::test]
async fn chat_and_user_binding_is_stored_with_the_command() {
    let store = InMemoryCommandStore::new();
    let (row, nonce) = TradeCommand::open(spec(TradeAction::Sell), at(10_000));
    store.insert(&row).await.unwrap();
    let found = store
        .find_by_nonce("org-1", 100, &nonce.sha256())
        .await
        .unwrap()
        .expect("found");
    assert_eq!(found.chat_id, 100);
    assert_eq!(found.user_id, 111);
    // The user id is part of the row, so a different requester is refused by
    // the handler (see buy_sell_flow.rs) — the row itself never accepts one.
    assert_ne!(found.user_id, 999);
    assert_eq!(found.percent_bps, Some(10_000));
    assert_eq!(found.amount_lamports, None);
}

#[tokio::test]
async fn cancel_and_revocation_cancel_pending() {
    let store = InMemoryCommandStore::new();
    let buy = TradeCommand::open(spec(TradeAction::Buy), at(10_000)).0;
    let sell = TradeCommand::open(spec(TradeAction::Sell), at(10_000)).0;
    let other_chat = TradeCommand::open(
        PendingSpec {
            chat_id: 101,
            ..spec(TradeAction::Buy)
        },
        at(10_000),
    )
    .0;
    for row in [&buy, &sell, &other_chat] {
        store.insert(row).await.unwrap();
    }
    assert_eq!(
        store.pending("org-1", 100, at(10_001)).await.unwrap().len(),
        2
    );

    // A single cancel touches exactly one command.
    assert!(store
        .transition(
            buy.command_id,
            CommandStatus::Pending,
            CommandStatus::Cancelled,
            at(10_001),
            None
        )
        .await
        .unwrap());
    assert_eq!(
        store.pending("org-1", 100, at(10_002)).await.unwrap().len(),
        1
    );

    // A consent revocation cancels everything pending for the chat only.
    let cancelled = store
        .cancel_pending("org-1", 100, at(10_003))
        .await
        .unwrap();
    assert_eq!(cancelled, vec![sell.command_id]);
    assert!(store
        .pending("org-1", 100, at(10_004))
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store.pending("org-1", 101, at(10_004)).await.unwrap().len(),
        1
    );
    // Idempotent: nothing left to cancel.
    assert!(store
        .cancel_pending("org-1", 100, at(10_005))
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn overdue_commands_expire_and_the_ledger_still_answers() {
    let store = InMemoryCommandStore::new();
    let (first, _) = TradeCommand::open(spec(TradeAction::Buy), at(10_000));
    let (second, _) = TradeCommand::open(spec(TradeAction::Buy), at(10_030));
    store.insert(&first).await.unwrap();
    store.insert(&second).await.unwrap();
    let expired = store.expire_overdue(at(10_061)).await.unwrap();
    assert_eq!(expired, vec![first.command_id]);
    assert_eq!(
        store.get(first.command_id).await.unwrap().unwrap().status,
        CommandStatus::Expired
    );
    assert_eq!(
        store.get(second.command_id).await.unwrap().unwrap().status,
        CommandStatus::Pending
    );
    assert!(store.expire_overdue(at(10_061)).await.unwrap().is_empty());
}

#[tokio::test]
async fn only_committed_buys_consume_the_daily_budget() {
    let store = InMemoryCommandStore::new();
    let (buy, _) = TradeCommand::open(spec(TradeAction::Buy), at(10_000));
    let (sell, _) = TradeCommand::open(spec(TradeAction::Sell), at(10_000));
    let (pending_buy, _) = TradeCommand::open(spec(TradeAction::Buy), at(10_000));
    for row in [&buy, &sell, &pending_buy] {
        store.insert(row).await.unwrap();
    }
    // A pending command has not committed anything yet.
    assert_eq!(store.spent_since("org-1", 100, at(0)).await.unwrap(), 0);
    store
        .transition(
            buy.command_id,
            CommandStatus::Pending,
            CommandStatus::Confirmed,
            at(10_001),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        store.spent_since("org-1", 100, at(0)).await.unwrap(),
        25_000_000
    );
    // A sell never touches the SOL budget.
    store
        .transition(
            sell.command_id,
            CommandStatus::Pending,
            CommandStatus::Confirmed,
            at(10_001),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        store.spent_since("org-1", 100, at(0)).await.unwrap(),
        25_000_000
    );
    // A failed command is no longer committed capacity.
    store
        .transition(
            buy.command_id,
            CommandStatus::Confirmed,
            CommandStatus::Failed,
            at(10_002),
            Some("risk_rejected"),
        )
        .await
        .unwrap();
    assert_eq!(store.spent_since("org-1", 100, at(0)).await.unwrap(), 0);
    // The window boundary excludes older rows.
    assert_eq!(
        store.spent_since("org-1", 100, at(10_002)).await.unwrap(),
        0
    );
    assert_eq!(
        store.count_since("org-1", 100, at(10_000)).await.unwrap(),
        3
    );
}

#[tokio::test]
async fn unknown_command_ids_are_not_silent_successes() {
    let store = InMemoryCommandStore::new();
    let ghost = Uuid::new_v4();
    assert!(store.get(ghost).await.unwrap().is_none());
    assert!(!store
        .transition(
            ghost,
            CommandStatus::Pending,
            CommandStatus::Confirmed,
            at(0),
            None
        )
        .await
        .unwrap());
    assert!(store
        .find_by_nonce("org-1", 100, &[0u8; 32])
        .await
        .unwrap()
        .is_none());
}
