//! BATCH 04 — the whole flow through the **real** dispatcher.
//!
//! These tests start `TelegramBot::run` for real (polling loop, allow-list,
//! `parse_command`, routing) against a local stand-in Bot API server, with a
//! [`FakeTradePort`] behind the console. Nothing is mocked inside the module:
//! the only substituted pieces are the network (a local HTTP server) and the
//! venue (the port, which is the design's seam).
//!
//! Covered: happy buy, happy sell, halted, live-gate closed, no consent,
//! unlinked chat, wrong user, port failure → failed + audit, and "no double
//! execution" on a repeated confirmation.

mod support;

use std::time::Duration;

use module_telegram::commands::pending::CommandStatus;
use module_telegram::{PortError, PortMode};
use support::{extract_nonce, Bot, CHAT, CHAT_B, LINKED_USER, ORG, OTHER_USER, UNLINKED_CHAT};

const MINT: &str = "So11111111111111111111111111111111111111112";

fn memory(bot: &Bot) -> &module_telegram::MemoryStores {
    bot.stores.memory().expect("in-memory stores")
}

async fn events(bot: &Bot) -> Vec<String> {
    let rows = bot.stores.audit.recent(ORG, 500).await.expect("audit rows");
    rows.into_iter()
        .map(|r| r.event.as_str().to_string())
        .collect()
}

#[tokio::test]
async fn happy_path_buy_is_requested_confirmed_and_executed_once() {
    let bot = Bot::start(true).await;
    bot.grant_consent().await;

    // 1. /buy creates a pending command and hands out a one-time nonce.
    let reply = bot
        .say(CHAT, LINKED_USER, &format!("/buy {MINT} 0.01 100"))
        .await;
    assert!(reply.contains("Pending BUY"), "{reply}");
    assert!(reply.contains("amount: 0.01 SOL"), "{reply}");
    assert!(reply.contains("mode: paper"), "{reply}");
    let nonce = extract_nonce(&reply).expect("nonce in reply");
    assert_eq!(nonce.len(), 32, "CSPRNG hex nonce: {nonce}");

    // Nothing has been executed yet.
    assert_eq!(bot.port.call_count(), 0);

    // 2. /confirm executes exactly once.
    let reply = bot
        .say(CHAT, LINKED_USER, &format!("/confirm {nonce}"))
        .await;
    assert!(reply.contains("Executed BUY"), "{reply}");
    assert_eq!(bot.port.call_count(), 1);
    assert_eq!(bot.port.accepted_order_count(), 1);
    let order = bot.port.accepted_orders()[0].clone();
    assert_eq!(order.organization_id, ORG);
    assert_eq!(order.amount_lamports, Some(10_000_000));
    assert_eq!(order.slippage_bps, 100);
    assert_eq!(order.idempotency_key, order.command_id);

    // 3. A duplicate /confirm is answered, never re-executed.
    let reply = bot
        .say(CHAT, LINKED_USER, &format!("/confirm {nonce}"))
        .await;
    assert!(reply.contains("already executed"), "{reply}");
    assert_eq!(
        bot.port.call_count(),
        1,
        "the port must not be called twice"
    );

    // 4. The audit trail holds exactly one row per transition.
    let audit = events(&bot).await;
    for expected in ["consent_granted", "requested", "confirmed", "executed"] {
        assert!(
            audit.iter().any(|e| e == expected),
            "missing audit event {expected} in {audit:?}"
        );
    }
    assert_eq!(
        audit.iter().filter(|e| *e == "executed").count(),
        1,
        "{audit:?}"
    );
    bot.stop().await;
}

#[tokio::test]
async fn happy_path_sell_executes_with_the_percentage() {
    let bot = Bot::start(true).await;
    bot.grant_consent().await;
    let reply = bot
        .say(CHAT, LINKED_USER, &format!("/sell {MINT} all 250"))
        .await;
    assert!(reply.contains("Pending SELL"), "{reply}");
    assert!(reply.contains("100% of the position"), "{reply}");
    let nonce = extract_nonce(&reply).expect("nonce");
    let reply = bot
        .say(CHAT, LINKED_USER, &format!("/confirm {nonce}"))
        .await;
    assert!(reply.contains("Executed SELL"), "{reply}");
    let order = bot.port.accepted_orders()[0].clone();
    assert_eq!(order.percent_bps, Some(10_000));
    assert_eq!(order.amount_lamports, None);
    assert_eq!(order.slippage_bps, 250);
    // A sell must not consume the SOL daily budget.
    let spent = bot
        .stores
        .commands
        .spent_since(ORG, CHAT, chrono::Utc::now() - chrono::Duration::days(1))
        .await
        .expect("spent");
    assert_eq!(spent, 0);
    bot.stop().await;
}

#[tokio::test]
async fn the_kill_switch_denies_and_is_audited() {
    let bot = Bot::start(true).await;
    bot.grant_consent().await;
    bot.state.set_kill_switch(true, "test").await;
    let reply = bot
        .say(CHAT, LINKED_USER, &format!("/buy {MINT} 0.01"))
        .await;
    assert!(reply.contains("halted"), "{reply}");
    assert_eq!(bot.port.call_count(), 0);
    assert!(events(&bot).await.iter().any(|e| e == "denied"));
    bot.state.set_kill_switch(false, "test").await;
    bot.stop().await;
}

#[tokio::test]
async fn a_halted_port_denies_the_request() {
    let bot = Bot::start(true).await;
    bot.grant_consent().await;
    bot.port.set_halted(true);
    let reply = bot
        .say(CHAT, LINKED_USER, &format!("/buy {MINT} 0.01"))
        .await;
    assert!(reply.contains("halted"), "{reply}");
    bot.port.set_halted(false);
    bot.stop().await;
}

#[tokio::test]
async fn live_with_a_closed_gate_is_refused() {
    let bot = Bot::start(true).await;
    bot.grant_consent().await;
    bot.port
        .set_mode(PortMode::new(bot_core::models::ExecutionMode::Live, false));
    let reply = bot
        .say(CHAT, LINKED_USER, &format!("/buy {MINT} 0.01"))
        .await;
    assert!(reply.contains("live_gates_closed"), "{reply}");
    assert_eq!(bot.port.call_count(), 0);

    // Both gates open ⇒ allowed again.
    bot.port
        .set_mode(PortMode::new(bot_core::models::ExecutionMode::Live, true));
    let reply = bot
        .say(CHAT, LINKED_USER, &format!("/buy {MINT} 0.01"))
        .await;
    assert!(reply.contains("Pending BUY"), "{reply}");
    assert!(reply.contains("mode: LIVE"), "{reply}");
    bot.stop().await;
}

#[tokio::test]
async fn without_consent_nothing_is_created() {
    let bot = Bot::start(true).await;
    let reply = bot
        .say(CHAT, LINKED_USER, &format!("/buy {MINT} 0.01"))
        .await;
    assert!(reply.contains("no_consent"), "{reply}");
    assert!(memory(&bot).commands.all().is_empty());
    bot.stop().await;
}

#[tokio::test]
async fn an_unlinked_chat_is_denied() {
    let bot = Bot::start(true).await;
    let reply = bot
        .say(UNLINKED_CHAT, LINKED_USER, &format!("/buy {MINT} 0.01"))
        .await;
    assert!(reply.contains("unlinked_chat"), "{reply}");
    assert_eq!(bot.port.call_count(), 0);
    // Even a consent grant from an unlinked chat is refused.
    let reply = bot
        .say(
            UNLINKED_CHAT,
            LINKED_USER,
            "/consent grant 0.01 0.05 24 I CONSENT TO TELEGRAM TRADING",
        )
        .await;
    assert!(reply.contains("unlinked_chat"), "{reply}");
    bot.stop().await;
}

#[tokio::test]
async fn a_different_telegram_user_cannot_use_a_linked_chat() {
    let bot = Bot::start(true).await;
    bot.grant_consent().await;
    let reply = bot
        .say(CHAT, OTHER_USER, &format!("/buy {MINT} 0.01"))
        .await;
    assert!(reply.contains("chat_user_mismatch"), "{reply}");
    assert!(memory(&bot).commands.all().is_empty());
    bot.stop().await;
}

#[tokio::test]
async fn a_port_failure_marks_the_command_failed_and_audits_it() {
    let bot = Bot::start(true).await;
    bot.grant_consent().await;
    let nonce = bot.request_buy("0.01").await;
    bot.port
        .fail_with(PortError::RiskRejected("max exposure".into()));
    let reply = bot
        .say(CHAT, LINKED_USER, &format!("/confirm {nonce}"))
        .await;
    assert!(reply.contains("NOT executed"), "{reply}");
    assert!(reply.contains("risk_rejected"), "{reply}");
    let row = memory(&bot).commands.all().first().cloned().expect("row");
    assert_eq!(row.status, CommandStatus::Failed);
    assert_eq!(row.failure_code.as_deref(), Some("risk_rejected"));
    assert_eq!(bot.port.accepted_order_count(), 0);
    assert!(events(&bot).await.iter().any(|e| e == "failed"));
    // A second confirmation of a failed command never calls the port again.
    let reply = bot
        .say(CHAT, LINKED_USER, &format!("/confirm {nonce}"))
        .await;
    assert!(reply.contains("already failed"), "{reply}");
    bot.stop().await;
}

#[tokio::test]
async fn caps_are_enforced_per_order() {
    let bot = Bot::start(true).await;
    bot.grant_consent().await;
    // The consent caps the per-order size at 0.05 SOL.
    let reply = bot
        .say(CHAT, LINKED_USER, &format!("/buy {MINT} 0.06"))
        .await;
    assert!(reply.contains("order_cap_exceeded"), "{reply}");
    assert!(memory(&bot).commands.all().is_empty());
    // At the cap is fine.
    let reply = bot
        .say(CHAT, LINKED_USER, &format!("/buy {MINT} 0.05"))
        .await;
    assert!(reply.contains("Pending BUY"), "{reply}");
    bot.stop().await;
}

#[tokio::test]
async fn the_rolling_daily_cap_stops_the_next_buy() {
    let bot = Bot::start(true).await;
    bot.grant_consent().await;
    // Confirmed buys accumulate: 0.05 + 0.05 + 0.05 = 0.15 ≤ 0.2, then 0.05
    // more would exceed the 0.2 SOL cap.
    for _ in 0..3 {
        let nonce = bot.request_buy("0.05").await;
        bot.say_expect(
            CHAT,
            LINKED_USER,
            &format!("/confirm {nonce}"),
            "Executed BUY",
        )
        .await;
    }
    let nonce = bot.request_buy("0.05").await;
    bot.say_expect(
        CHAT,
        LINKED_USER,
        &format!("/confirm {nonce}"),
        "Executed BUY",
    )
    .await;
    // 0.20 SOL is now committed; the next order must be refused.
    let reply = bot
        .say(CHAT, LINKED_USER, &format!("/buy {MINT} 0.01"))
        .await;
    assert!(reply.contains("daily_cap_exceeded"), "{reply}");
    bot.stop().await;
}

#[tokio::test]
async fn cancel_and_pending_are_honest() {
    let bot = Bot::start(true).await;
    bot.grant_consent().await;
    bot.say_expect(CHAT, LINKED_USER, "/pending", "No pending commands")
        .await;
    let nonce = bot.request_buy("0.01").await;
    let reply = bot.say(CHAT, LINKED_USER, "/pending").await;
    assert!(reply.contains("Pending commands for this chat"), "{reply}");
    assert!(reply.contains("BUY 0.01 SOL"), "{reply}");
    bot.say_expect(CHAT, LINKED_USER, &format!("/cancel {nonce}"), "Cancelled")
        .await;
    // Confirming a cancelled nonce is refused, not executed.
    let reply = bot
        .say(CHAT, LINKED_USER, &format!("/confirm {nonce}"))
        .await;
    assert!(reply.contains("already cancelled"), "{reply}");
    assert_eq!(bot.port.call_count(), 0);
    bot.stop().await;
}

#[tokio::test]
async fn revoking_the_consent_cancels_pending_and_blocks_new_orders() {
    let bot = Bot::start(true).await;
    bot.grant_consent().await;
    let nonce = bot.request_buy("0.01").await;
    let reply = bot.say(CHAT, LINKED_USER, "/consent revoke").await;
    assert!(reply.contains("Consent revoked"), "{reply}");
    assert!(reply.contains("1 pending command"), "{reply}");
    let reply = bot
        .say(CHAT, LINKED_USER, &format!("/confirm {nonce}"))
        .await;
    assert!(reply.contains("already cancelled"), "{reply}");
    assert_eq!(bot.port.call_count(), 0);
    let reply = bot
        .say(CHAT, LINKED_USER, &format!("/buy {MINT} 0.01"))
        .await;
    assert!(
        reply.contains("consent_revoked") || reply.contains("no_consent"),
        "{reply}"
    );
    bot.stop().await;
}

#[tokio::test]
async fn consent_status_reports_the_caps() {
    let bot = Bot::start(true).await;
    let reply = bot.say(CHAT, LINKED_USER, "/consent status").await;
    assert!(reply.contains("No consent is on file"), "{reply}");
    bot.grant_consent().await;
    let reply = bot.say(CHAT, LINKED_USER, "/consent status").await;
    assert!(reply.contains("status: active"), "{reply}");
    assert!(reply.contains("per-order cap: 0.05 SOL"), "{reply}");
    assert!(reply.contains("24h cap: 0.2 SOL"), "{reply}");
    assert!(reply.contains("scope: buy, sell"), "{reply}");
    bot.stop().await;
}

#[tokio::test]
async fn a_second_grant_is_refused_until_revoked() {
    let bot = Bot::start(true).await;
    bot.grant_consent().await;
    let reply = bot
        .say(
            CHAT,
            LINKED_USER,
            "/consent grant 0.01 0.05 24 I CONSENT TO TELEGRAM TRADING",
        )
        .await;
    assert!(reply.contains("consent_already_active"), "{reply}");
    bot.stop().await;
}

#[tokio::test]
async fn a_wrong_phrase_never_grants() {
    let bot = Bot::start(true).await;
    let reply = bot
        .say(CHAT, LINKED_USER, "/consent grant 0.01 0.05 24 yes please")
        .await;
    assert!(reply.contains("phrase_mismatch"), "{reply}");
    assert!(memory(&bot).consents.all().is_empty());
    bot.stop().await;
}

#[tokio::test]
async fn a_grant_above_the_org_limit_is_refused() {
    let bot = Bot::start(true).await;
    // The fake port reports a 1 SOL per-order ceiling for org-1.
    bot.port.set_org_limits(
        ORG,
        module_telegram::OrgRiskLimits::new(1_000_000_000, 4_000_000_000),
    );
    let reply = bot
        .say(
            CHAT,
            LINKED_USER,
            "/consent grant 2 2 24 I CONSENT TO TELEGRAM TRADING",
        )
        .await;
    assert!(reply.contains("exceeds_org"), "{reply}");
    assert!(memory(&bot).consents.all().is_empty());
    bot.stop().await;
}

#[tokio::test]
async fn chats_have_independent_consents_and_rate_buckets() {
    let bot = Bot::start(true).await;
    bot.grant_consent().await;
    // The second linked chat has no consent of its own.
    let reply = bot
        .say(CHAT_B, LINKED_USER, &format!("/buy {MINT} 0.01"))
        .await;
    assert!(reply.contains("no_consent"), "{reply}");
    // Granting there is independent.
    let reply = bot
        .say(
            CHAT_B,
            LINKED_USER,
            "/consent grant 0.01 0.05 24 I CONSENT TO TELEGRAM TRADING",
        )
        .await;
    assert!(reply.contains("Consent granted"), "{reply}");
    let nonce = extract_nonce(
        &bot.say(CHAT_B, LINKED_USER, &format!("/buy {MINT} 0.01"))
            .await,
    )
    .expect("nonce");
    // A nonce from chat B cannot be confirmed from chat A.
    let reply = bot
        .say(CHAT, LINKED_USER, &format!("/confirm {nonce}"))
        .await;
    assert!(reply.contains("unknown_nonce"), "{reply}");
    bot.say_expect(
        CHAT_B,
        LINKED_USER,
        &format!("/confirm {nonce}"),
        "Executed BUY",
    )
    .await;
    bot.stop().await;
}

#[tokio::test]
async fn malformed_commands_are_explained_not_executed() {
    let bot = Bot::start(true).await;
    bot.grant_consent().await;
    let cases = [
        (format!("/buy {MINT} 0"), "amount_zero"),
        (format!("/buy {MINT} -1"), "amount_negative"),
        (format!("/buy {MINT} 1e5"), "amount_not_numeric"),
        (
            format!("/buy {MINT} 0.0000000001"),
            "amount_too_many_decimals",
        ),
        ("/buy notamint 0.01".to_string(), "mint_wrong_length"),
        (format!("/buy {MINT} 0.01 0"), "slippage_out_of_range"),
        (format!("/buy {MINT} 0.01 501"), "slippage_out_of_range"),
        (format!("/sell {MINT} 0"), "percent_out_of_range"),
        (format!("/sell {MINT} 100.01"), "percent_out_of_range"),
        ("/confirm zzz".to_string(), "malformed_nonce"),
        (
            "/confirm 0123456789abcdef0123456789abcdef".to_string(),
            "unknown_nonce",
        ),
        (
            "/cancel 0123456789abcdef0123456789abcdef".to_string(),
            "unknown_nonce",
        ),
    ];
    for (text, expected) in cases {
        let reply = bot.say(CHAT, LINKED_USER, &text).await;
        assert!(
            reply.contains(expected),
            "{text:?} should be refused with {expected}, got: {reply:?}"
        );
    }
    assert_eq!(bot.port.call_count(), 0);
    bot.stop().await;
}

#[tokio::test]
async fn the_rate_limiter_engages_after_the_bucket_is_empty() {
    let bot = Bot::start(true).await;
    bot.grant_consent().await;
    // Capacity is 5 commands per 60s (the default bucket).
    for _ in 0..5 {
        let reply = bot
            .say(CHAT, LINKED_USER, &format!("/buy {MINT} 0.001"))
            .await;
        assert!(reply.contains("Pending BUY"), "{reply}");
    }
    let reply = bot
        .say(CHAT, LINKED_USER, &format!("/buy {MINT} 0.001"))
        .await;
    assert!(reply.contains("rate_limited"), "{reply}");
    // A different chat is unaffected.
    let reply = bot.say(CHAT_B, LINKED_USER, "/pending").await;
    assert!(!reply.contains("rate_limited"), "{reply}");
    bot.stop().await;
}

#[tokio::test]
async fn a_readonly_role_cannot_trade() {
    let bot = Bot::start(true).await;
    bot.grant_consent().await;
    // Promote `OTHER_USER` to readonly, then link the chat to that user.
    bot.state
        .update_config(|cfg| {
            cfg.telegram.readonly_user_ids = vec![OTHER_USER];
            cfg.telegram.owner_user_ids = vec![LINKED_USER];
        })
        .await;
    // The chat is still linked to LINKED_USER, so OTHER_USER is refused by the
    // role gate before the link check.
    let reply = bot
        .say(CHAT, OTHER_USER, &format!("/buy {MINT} 0.01"))
        .await;
    assert!(reply.contains("insufficient_role"), "{reply}");
    bot.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_confirmations_execute_at_most_once() {
    let bot = Bot::start(true).await;
    bot.grant_consent().await;
    let nonce = bot.request_buy("0.01").await;
    let mut handles = Vec::new();
    for _ in 0..4 {
        let chat = CHAT;
        let user = LINKED_USER;
        let nonce = nonce.clone();
        let console = std::sync::Arc::clone(bot.console.as_ref().expect("console"));
        handles.push(tokio::spawn(async move {
            console
                .handle(
                    module_telegram::commands::Command::Confirm(nonce),
                    module_telegram::commands::TgRole::Owner,
                    chat,
                    Some(user),
                    chrono::Utc::now(),
                )
                .await
                .map(|reply| reply.plain().to_string())
        }));
    }
    let mut executed = 0;
    for handle in handles {
        let reply = handle.await.expect("join").expect("reply");
        if reply.contains("Executed BUY") {
            executed += 1;
        }
    }
    assert_eq!(executed, 1, "exactly one confirmation may execute");
    assert_eq!(bot.port.call_count(), 1);
    tokio::time::sleep(Duration::from_millis(50)).await;
    bot.stop().await;
}
